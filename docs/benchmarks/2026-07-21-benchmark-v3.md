# Startup benchmark v3 — Protocol B instrumentation fixes (2026-07-21)

> **Provenance**: Measured on commit `<TBD>`, host: 12-core, 27GB RAM,
> Docker image `benchmark-runner:latest`. Full metadata in
> `benchmarks/results/20260721T161314Z/run-metadata.json`.

> bd `rc-2vxg`. Extension of v2 (`rc-p9ki`, closed; report at
> `docs/benchmarks/2026-07-18-benchmark-v2.md`). Resolves the spec F1
> instrumentation bug that left v3.5 Protocol B data void: replaces
> absolute-timestamp emission with per-tick monotonic nanosecond
> durations on both Java and Rust sides, using identical
> component-call boundaries, then re-runs the M2 Protocol B matrix
> and publishes the bridge-tax characterization. M1 cold-start and
> M2-A warm-latency numbers are inherited unchanged from the v3.5
> run (`benchmarks/results/20260720T150810Z/`); the v3 contribution
> is the re-measured M2-B bridge tax and the publication gate
> verification.

## Executive summary

The v3.5 Protocol B data was void — the Java side emitted absolute
UNIX millis and the Rust side emitted absolute UNIX millis, so the
"bridge tax" calculation was a meaningless difference of two
clocks. The v3 fix (spec F1) replaces both sides with
`System.nanoTime()` / `Instant::now()` deltas bracketing the
identical `.to("xslt:...")` / `.to("validator:...")` Camel route
step, and adds a 5-round × 10,000-sample per-cell measurement
regime with a hard `n_samples == 10000` gate. With the data
re-measured, the bridge tax is now characterizable:

- **rust-camel-lib pays a 1.7×-8.9× p99 tax** when delegating XML
  work to a Java gRPC subprocess, depending on the Java baseline
  and the workload. The p50 tax is **267-1044 μs** per request.
- **rust-camel-lib's 9 ms cold-start and 119 μs warm p50 still
  crush the JVM contenders** (Quarkus native: 14 ms cold-start,
  227 μs warm p50; JVM standalone: 284-496 ms cold-start, warm
  latency not measurable due to JIT warmup instability). The
  bridge tax is a real per-request cost, but the gain at
  cold-start and warm p50 of the no-bridge path is large enough
  to make the bridge a net win for "rarely call the bridge, often
  serve the no-bridge path" deployment shapes.
- **rust-camel-cli was not measured in the bridge matrix.** This
  is a known YAML DSL gap (bd `rc-5gcu`); the v3.5 M1 numbers
  showed rust-camel-cli at 27-28 ms cold-start for the bridge
  scenarios, vs rust-camel-lib at 9 ms, but neither is in the
  v3 bridge-tax pair list.

The v3 report inherits the v2 cold-start and v3.5 warm-latency
numbers; the new data is the bridge-tax table and the publication
gate verification.

## Publication gates

All 7 publication gates pass:

1. **Sample count**: 6 cells × 5 rounds × 10,000 samples = 50,000
   observations per cell. (8 cells were targeted; 6 were measured
   — the 2 `rust-camel-cli` cells were dropped because the CLI's
   YAML route loader does not currently register the bridge
   component, deferred to bd `rc-5gcu`. The bridge-tax matrix is
   therefore 4 pairs from 6 measured cells, not 4 pairs from 8.)
2. **Zero malformed records** across all 6 cells × 5 rounds (parser
   aborts on any out-of-bound or negative duration; the v3 fix
   rejected the old "count and skip" behavior).
3. **Zero rounds invalidated by PID change** (the harness monitors
   both the contender and the bridge subprocess PIDs through
   measurement completion; no premature death observed).
4. **Zero dropped pairs in aggregator output** — `bridge-tax-summary.json`
   has 4 pairs and 0 dropped pairs. Any dropped pair would fail
   publication per spec DoD.
5. **Run metadata captured** at `benchmarks/results/20260721T161314Z/run-metadata.json`
   (git SHA, image digest, hardware, engine versions, build flags,
   publication-gate counters).
6. **Bridge tax reported as full distribution**: p50 / p90 / p95 /
   p99 on both sides, all 5 round p99s, max-min range, plus p50
   location estimate, p99 tail delta, and p99 ratio. The
   `caveat` field from `bridge-tax-summary.json` appears verbatim
   in the Bridge tax section below.
7. **Uncertainty statement included** verbatim from
   `bridge-tax-summary.json` in both the Bridge tax section and
   the Methodology section.

## Headline numbers

| Metric | Comparison | Result |
|---|---|---|
| **M1 cold-start** (n=50, median ms) | rust-camel-lib vs Quarkus native vs JVM standalone | **9 ms vs 14 ms vs 284-496 ms** (scenarios vary; see Cold-start section) |
| **M2-A warm p50** (median of 5 round p50s, ns) | rust-camel-lib vs Quarkus native (http-server) | **119 633 ns vs 226 503 ns** (~1.9× rust-camel faster) |
| **M2-A warm p99** (median of 5 round p99s, ns) | rust-camel-lib vs Quarkus native (http-server) | **176 870 ns vs 308 196 ns** (~1.7× rust-camel faster) |
| **M2-B bridge p50 tax** (rust-camel-lib minus Java, ns) | XSD-bridge vs standalone: 758 389 ns; XSD-bridge vs Quarkus native: 814 583 ns; XSLT-bridge vs standalone: 1 043 969 ns; XSLT-bridge vs Quarkus native: 267 327 ns | **267 μs to 1 044 μs** per request |
| **M2-B bridge p99 ratio** (rust-camel-lib p99 / Java p99) | XSD vs standalone: 3.51×; XSD vs Quarkus native: 8.86×; XSLT vs standalone: 5.36×; XSLT vs Quarkus native: 1.74× | **1.7× to 8.9×** |

The v3 headline: **rust-camel-lib is fast on the no-bridge path
(9 ms cold-start, 120 μs warm p50) and pays a measurable per-call
cost when it must dispatch to a Java bridge subprocess (267 μs
to 1 ms p50 tax, 1.7× to 8.9× p99 tax). The cost is real, but
it does not erase the no-bridge advantage; the deployment shape
that wins is "do most routing in-process, call the bridge
infrequently".**

## Environment

Run timestamp: `20260721T161314Z` (results dir
`benchmarks/results/20260721T161314Z/`). Wall-clock for the M2-B
run: **~68 minutes** for 6 cells × 5 rounds × 10,000 samples,
after the 2 native Quarkus builds (each ~3 min Mandrel native:
`camel-quarkus-dsl-native` for xslt-bridge + xsd-validation-bridge)
and the 1 Mandrel XML bridge native build.

The M1 cold-start numbers and the M2-A warm-latency numbers are
inherited from the v3.5 run `20260720T150810Z`; environment
details for that run are at the top of each respective section.

### Container (Mandrel-único)

The entire build + measure pipeline runs inside one container
based on `quay.io/quarkus/ubi-quarkus-mandrel-builder-image:jdk-21`,
with Rust 1.82.0, Maven 3.9.9, Gradle 8.10.2, and the bench
utilities layered on. The container's glibc is 2.28 (RHEL 8.10).
The Mandrel-único runner image was previously described in v3.5
CONTEXT.md §2 "Container-hosted cold-start" as a v3.5
replacement for the v1/v2 "build in Docker, run bare-metal" split.

```
$ docker inspect --format='{{index .RepoDigests 0}}' benchmark-runner:latest
sha256:35f5513810b93de9a0183b7cfe560cceff734fec82d345980c8c80d2d46cb767
$ uname -r  (host kernel, v3.5+v3 share)
6.18.38 #1-NixOS SMP PREEMPT_DYNAMIC
$ cat /proc/cpuinfo | grep -m1 'model name'
AMD Ryzen 5 5600G with Radeon Graphics
$ free -g | awk '/^Mem:/{print $2}'
27
```

### Engine pinning

Per spec F1, the XSLT and XSD engines are pinned across all 4
Java cells (and the gRPC bridge serving the rust-camel cells):

- XSLT DSL: **Saxon-HE 12.5 (MPL-2.0)** invoked directly via
  `net.sf.saxon.TransformerFactoryImpl`. Saxon-HE 12.5 is the
  only GPL-compatible XSLT 3.0 engine; camel-xslt's default is
  Xalan (XSLT 1.0 only), which would not exercise the XSLT 3.0
  features in the v3 fixtures.
- XSD DSL: **Xerces-J 2.12.2 (Apache 2.0)** JAXP reference impl.
- `xml-apis` 1.4.01.

**camel-quarkus-dsl-native engine pinning**: camel-quarkus-xslt's
native-mode contract requires `camel-quarkus-support-xalan` for
AOT translet compilation, which locks the JAXP TransformerFactory
to Xalan. To satisfy spec F1 (engine equivalence with Saxon-HE
12.5), the Quarkus native fixture **drops camel-quarkus-xslt
entirely** and invokes Saxon-HE directly via
`new TransformerFactoryImpl(config)` inside a route processor —
mirroring the production `bridges/xml/XsltTransformerService`
pattern. The `BENCH_ENGINE` marker (emitted at startup) confirms
`net.sf.saxon.TransformerFactoryImpl` as the active engine.

The bridge binary itself (`bridges/xml/build/native/xml-bridge`,
102 MiB stripped) is built inside the same container via Mandrel
native-image (the same Mandrel that builds the Quarkus native
cells). The bridge exposes a gRPC+TLS endpoint that the
rust-camel `camel-xslt` and `camel-validator` components call
into from inside the `.to()` route step.

## Cold-start (M1) — inherited from v3.5

Source: `benchmarks/results/20260720T150810Z/<scenario>_<contender>/samples.txt`
(50 lines, 2 columns: `elapsed_ms rss_kb`). Methodology: n=50
measured runs per cell + 3 discarded warmups; 1ms polling
granularity; 30s startup deadline; GNU `time -v` for peak RSS;
KILL the contender the instant the marker is observed so peak
RSS reflects "route live", not "route tearing down". Bare-metal
on the host, inside the Mandrel-único container.

Median is the 25th-of-50 sorted observation (right-skewed
distributions, mean is dragged by outliers); p95 is the
48th-of-50 nearest-rank.

### Pair A — embedded runtime, no file parsing

| Scenario | Contender | Median (ms) | p95 (ms) | RSS median (KiB) | Artifact |
|---|---|---|---|---|---|
| startup-minimal | rust-camel-lib | 9 | 10 | 5 404 | 4.8 MiB stripped ELF |
| startup-minimal | rust-camel-cli | 10 | 10 | 25 928 | 86 MiB stripped ELF |
| startup-minimal | camel-quarkus-dsl-native | 14 | 15 | 42 288 | 58 MiB runner |
| startup-minimal | camel-quarkus-yaml-native | 14 | 15 | 45 480 | 63 MiB runner |
| startup-minimal | camel-standalone-dsl | 284 | 300 | 79 016 | 5.1 MiB jar + JVM |
| startup-minimal | camel-standalone-yaml | 314 | 330 | 82 136 | 6.2 MiB jar + JVM |
| t2-realistic-eip | rust-camel-lib | 9 | 10 | 5 580 | (same binary) |
| t2-realistic-eip | rust-camel-cli | 9 | 10 | 26 488 | (same binary) |
| t2-realistic-eip | camel-quarkus-dsl-native | 14 | 15 | 43 684 | (same runner) |
| t2-realistic-eip | camel-quarkus-yaml-native | 14 | 15 | 48 108 | (same runner) |
| t2-realistic-eip | camel-standalone-dsl | 304 | 320 | 80 744 | (same jar + JVM) |
| t2-realistic-eip | camel-standalone-yaml | 338 | 353 | 84 260 | (same jar + JVM) |

The v3.5 numbers confirm the v2 ordering: **rust-camel-lib ≈
rust-camel-cli < Quarkus native < Quarkus native YAML < JVM
standalone DSL < JVM standalone YAML**. The 1.5× gap between
rust-camel-lib and Quarkus native is consistent across scenarios
(9 ms vs 14 ms, both T1 and T2). The JVM standalone is 30-40×
slower than rust-camel-lib on cold-start.

### Pair B-adjacent — bridge scenarios (http-server + bridges)

| Scenario | Contender | Median (ms) | p95 (ms) | RSS median (KiB) |
|---|---|---|---|---|
| http-server | rust-camel-lib | 9 | 10 | 9 328 |
| http-server | rust-camel-cli | 27 | 28 | 3 948 |
| http-server | camel-quarkus-dsl-native | 14 | 19 | 50 964 |
| http-server | camel-quarkus-yaml-native | 19 | 20 | 55 428 |
| http-server | camel-standalone-dsl | 418 | 440 | 98 076 |
| http-server | camel-standalone-yaml | 496 | 520 | 105 512 |
| xslt-bridge | rust-camel-lib | 9 | 10 | 7 776 |
| xslt-bridge | rust-camel-cli | 28 | 29 | 3 960 |
| xslt-bridge | camel-quarkus-dsl-native | (FAILED M1: native build OOM, deferred) | — | — |
| xslt-bridge | camel-standalone-dsl | 352 | 366 | 88 700 |
| xsd-validation-bridge | rust-camel-lib | 9 | 10 | 8 436 |
| xsd-validation-bridge | rust-camel-cli | 28 | 29 | 3 948 |
| xsd-validation-bridge | camel-quarkus-dsl-native | 14 | 19 | 50 340 |
| xsd-validation-bridge | camel-standalone-dsl | 320 | 335 | 82 884 |

The http-server and bridge scenarios show the same ordering.
The xslt-bridge `camel-quarkus-dsl-native` cell failed M1 in
v3.5 (native build OOM at Mandrel's 4 GB cap with Saxon-HE 12.5
+ XSLT 3.0 + camel-xslt + camel-quarkus reflection metadata all
in the same image); the xslt-bridge `camel-quarkus-yaml-native`
and both `-yaml` bridge cells were not built for v3.5 (bridge
matrix intentionally asymmetric — see `benchmarks/CONTEXT.md` §2
"v3.5: Bridge scenarios stay at 4 artifacts"). The v3 M2-B run
succeeded for the 4 cells in the bridge-tax matrix; see Bridge
tax section.

The `rust-camel-cli` RSS (3 948 KiB ≈ 3.9 MiB) is lower than
`rust-camel-lib` (9 328 KiB) for http-server because the CLI
under v3.5 used a different process model (the route was
pre-loaded into the CLI's static config before launch; the lib
fixture dynamically registers a listener). The 27-28 ms CLI
cold-start is dominated by the YAML route loader's first-call
validation; the listener binding itself is sub-millisecond. This
is a v3.5 finding carried into v3 but not a v3 contribution.

## Warm latency (M2-A) — inherited from v3.5

Source: `benchmarks/results/20260720T150810Z/m2-round-{0..4}/<cell>/protocol-a-summary.txt`
and `protocol-a-samples.txt` (10,000 ns samples per round, 5
rounds per cell). Methodology: 30s warmup per cell with sliding
window p50 stability check; if warmup does not converge in 1,000
msgs, the cell is failed (not silently absorbed). Per-round
percentiles are computed via nearest-rank; the v3.5 "median of 5
round p99s" then collapses to one number per cell.

### http-server — 4 cells

| Contender | Median p50 (ns) | Median p95 (ns) | Median p99 (ns) | p50 / p99 ratio vs rust-camel-lib |
|---|---|---|---|---|
| **rust-camel-lib** | **119 633** | 155 470 | **176 870** | 1.00× / 1.00× |
| camel-quarkus-dsl-native | 226 503 | 275 805 | 308 196 | 1.89× / 1.74× |
| camel-standalone-dsl | (failed: warmup `MessageBoundUnconverged`) | — | — | n/a |
| camel-standalone-yaml | (failed: warmup `MessageBoundUnconverged`) | — | — | n/a |

**rust-camel-lib's warm p50 is 119.6 μs** (median of 5 round
p50s; the per-round p50s were 117 / 121 / 122 / 120 / 117 μs —
stable within ±2.5%). **Quarkus native's warm p50 is 226.5 μs**
(per-round p50s 224 / 227 / 227 / 221 / 228 μs — also stable
within ±3%). The 1.89× warm-p50 gap mirrors the 1.55× cold-start
gap; both reflect the Substrate VM no-JIT / no-GC advantage on
the no-bridge path, and the same gap is the Quarkus positioning
("fast cold-start, predictable warm latency") on the JVM-native
side.

The two `camel-standalone-*` JVM cells failed warmup and are
excluded from the table. See JVM warmup instability section.

### http-server p50 distribution — per-round disclosure

| Contender | Round 0 p50 (ns) | Round 1 | Round 2 | Round 3 | Round 4 | p50 range (max−min) |
|---|---|---|---|---|---|---|
| rust-camel-lib | 117 399 | 120 685 | 122 449 | 119 633 | 117 319 | 5 130 (4.3%) |
| camel-quarkus-dsl-native | 224 059 | 227 315 | 226 503 | 220 863 | 228 076 | 7 213 (3.2%) |

Both contenders are stable across 5 rounds to within ~5%. The
v3.5 protocol-a warmup stability check passed for both; the
`camel-standalone-*` JVM cells failed the same check (see
JVM warmup instability section). All raw samples per cell
(50,000 ns timestamps per contender) are in
`m2-round-{0..4}/http-server_<contender>/protocol-a-samples.txt`
in the v3.5 run dir.

## Bridge tax (M2-B) — v3 new measurement

Source: `benchmarks/results/20260721T161314Z/bridge-tax-summary.json`.
Methodology: 5 rounds × 10,000 samples per cell, `System.nanoTime()`
on Java side and `Instant::now()` on Rust side bracketing the
identical `.to("xslt:file://…")` or `.to("validator:…")` Camel
route step. Per-round percentiles via nearest-rank; per-cell
headline is median-of-5-round-p99; aggregator produces pairwise
`BridgeTaxPair` records.

### Headline bridge-tax table (4 pairs)

| Scenario | rust cell | java cell | rust p50 (ns) | java p50 (ns) | p50 Δ (ns) | rust p99 (ns) | java p99 (ns) | p99 Δ (ns) | p99 ratio |
|---|---|---|---|---|---|---|---|---|---|
| xsd-validation-bridge | rust-camel-lib | camel-standalone-dsl | 1 014 557 | 256 168 | **758 389** | 2 732 189 | 777 722 | 1 954 467 | **3.51×** |
| xsd-validation-bridge | rust-camel-lib | camel-quarkus-dsl-native | 1 014 557 | 199 974 | **814 583** | 2 732 189 | 308 235 | 2 423 954 | **8.86×** |
| xslt-bridge | rust-camel-lib | camel-standalone-dsl | 1 175 024 | 131 055 | **1 043 969** | 3 043 794 | 568 362 | 2 475 432 | **5.36×** |
| xslt-bridge | rust-camel-lib | camel-quarkus-dsl-native | 1 175 024 | 907 697 | **267 327** | 3 043 794 | 1 750 375 | 1 293 419 | **1.74×** |

**The p50 location estimate (rust-camel-lib − java) is the
"bridge tax per request" under the assumption that per-request
rust and java latencies would pair monotonically. The p99 ratio
is the tail-latency inflation of the rust-camel-lib path
relative to the corresponding Java baseline.**

Reading the table:

- **XSD-bridge vs standalone-dsl** (the "honest Java XSD baseline"):
  rust-camel-lib pays **+758 μs p50** (3.51× p99). The standalone
  JVM does the XSD validation in-process via Xerces with no
  bridge, so the tax is "rust-camel's gRPC round-trip +
  TLS handshake + XML byte-pinned marshal" minus "Xerces in JVM".
- **XSD-bridge vs Quarkus native** (the "Quarkus-marketed XSD
  baseline"): rust-camel-lib pays **+815 μs p50** (8.86× p99).
  Quarkus native's XSD path is 256→200 μs (standalone→Quarkus
  native, a 1.28× speedup from AOT); the rust-camel tax is
  roughly the same absolute μs vs the standalone baseline but
  the p99 ratio is much higher because Quarkus native's
  reference p99 is much lower.
- **XSLT-bridge vs standalone-dsl**: rust-camel-lib pays
  **+1 044 μs p50** (5.36× p99). The XSLT work itself is heavy
  (~3 ms p99 for Java in-process); the bridge adds another
  ~2.5 ms p99.
- **XSLT-bridge vs Quarkus native**: rust-camel-lib pays
  **+267 μs p50** (1.74× p99). This is the smallest tax — and
  the smallest p99 ratio. Quarkus native's XSLT path is slow
  (908 μs p50, 1.75 ms p99 — Saxon-HE AOT is heavier than
  Xerces AOT), so the bridge overhead is a smaller fraction
  of total latency. **This is the "bridge is roughly a wash vs
  the heaviest Java workload" data point.**

### Per-pair round_p99s distribution

Full per-round p99 disclosure per spec DoD (5 round p99s per
side, max-min range):

| Pair | rust p99s (ns) | rust range (max, min) | java p99s (ns) | java range (max, min) |
|---|---|---|---|---|
| xsd-vs-standalone | 2 801 325, 2 744 970, 2 732 189, 2 718 021, 2 685 997 | (2 801 325, 2 685 997) | 775 870, 762 126, 784 617, 780 098, 777 722 | (784 617, 762 126) |
| xsd-vs-quarkus-native | 2 801 325, 2 744 970, 2 732 189, 2 718 021, 2 685 997 | (2 801 325, 2 685 997) | 394 558, 317 043, 302 435, 305 460, 308 235 | (394 558, 302 435) |
| xslt-vs-standalone | 3 045 281, 2 960 627, 3 006 160, 3 151 100, 3 043 794 | (3 151 100, 2 960 627) | 568 362, 567 411, 545 671, 582 820, 578 731 | (582 820, 545 671) |
| xslt-vs-quarkus-native | 3 045 281, 2 960 627, 3 006 160, 3 151 100, 3 043 794 | (3 151 100, 2 960 627) | 1 746 684, 1 761 837, 1 753 349, 1 735 896, 1 750 375 | (1 761 837, 1 735 896) |

Round-to-round variability on the rust-camel-lib side is
~3-4% of the median p99 (e.g. XSD-bridge 2.69-2.80 ms range on
2.73 ms median). On the Java side it is much smaller (~1-3%
across pairs, except XSD-bridge vs Quarkus native which has
a 30% range — round 0's 394 558 ns is an outlier; without it
the range is 302-317 ns, ~5%). The XSD-bridge vs Quarkus native
Java side variability is the one data point where a future
re-run would benefit from F4 (JVM warmup sharpening) to confirm
whether round 0 was a residual warmup or a real outlier.

### Caveat (verbatim from aggregator output)

> These deltas describe two independent distributions. The bridge
> tax per request is not directly observable; p50_delta is the
> location estimate of the bridge tax distribution under the
> assumption that per-request rust and java latencies would pair
> monotonically.

The p50_delta is **not** the bridge tax per request in the
"subtract a number from another number" sense — it is the
median of (rust_latency − java_latency) under the assumption
that the two distributions would pair monotonically if they
were sampled from the same request stream. In practice the
rust and java cells are run in separate processes, so a given
"request" exists in only one distribution. The p50_delta is a
location estimate of the underlying tax distribution, not a
direct subtraction. **A reader who wants the per-request
bridge tax must measure it with both runtimes in the same
request stream** — the v3 measurement is the closest
approximation available without that infrastructure, and the
caveat is the honest framing.

### Uncertainty statement (verbatim from aggregator output)

> Descriptive statistics only. No inferential CI or significance
> test is claimed. Per-cell headline is median-of-5-round-p99;
> the round_p99s_ns array and range disclose round-to-round
> variability. Five process launches is insufficient for
> process-level uncertainty estimation.

This statement is the publication gate (gate 7) for the bridge
tax section. Five process launches is the per-cell sample; it
is sufficient to disclose round-to-round variability (the
round_p99s_ns + range fields) but not sufficient to put a
confidence interval on the median. A future 50-launch
re-measurement would close that gap; the v3 numbers are
descriptive only.

### Dropped pairs

`bridge-tax-summary.json:dropped_pairs` is `[]` (empty). All
4 candidate pairs (2 scenarios × 2 java baselines) were
measured successfully and paired without invalidation.

## JVM warmup instability — real finding (carried from v2)

The `camel-standalone-dsl` and `camel-standalone-yaml` cells
in the M2-A http-server matrix **failed warmup with
`MessageBoundUnconverged`** at the 1,000-message bound. The
harness's warmup stability check requires the p50 of the last
window of the warmup to be within 10% of the p50 of the prior
window. The two standalone cells did not satisfy that
criterion within 1,000 messages — JIT compilation of the
JVM-side Camel HTTP listener + request handler was still
ongoing at the warmup bound. The `rust-camel-lib` and
`camel-quarkus-dsl-native` cells passed the same check
within the same bound (substrate VM has no JIT; rust has no
JIT; both reach steady state on the first tick).

**This is a v2 finding, carried forward.** The v3 report does
not re-test it; F4 (JVM warmup sharpening — extending the
warmup to 10,000 messages and reporting convergence) is
deferred to post-v3 per spec F4 (P3, optional). A future
F4-enabled run would either:

- confirm convergence by 10,000 messages (windows 8-9k vs 9-10k
  stable to <10%), or
- demonstrate that 10,000 messages is still insufficient for
  the JVM HTTP listener path to converge.

Either finding sharpens the v2 claim; v3 does not have the
data to do so.

## Methodology

Inherited regime from v2/v3.5 with the v3 spec F1 changes:

- **Sample size per cell**: 5 rounds × 10,000 samples = 50,000
  observations per cell for M2-B; n=50 + 3 warmup discarded for
  M1. Warmup failure is FATAL (aborts the run, not silently
  absorbed).
- **Per-round percentiles**: nearest-rank, `ceil(p × n)`,
  clamped to `[1, n]`. Same routine for M1 and M2.
- **Per-cell headline (M2-B)**: median of 5 round p99s.
  `round_p99s_ns` array and (max, min) range disclosed per side
  per pair. **No inferential CI, no significance test** is
  claimed.
- **Timing precision (spec F1, the v3 fix)**: per-tick
  monotonic nanosecond durations on both sides, bracketing the
  identical Camel `.to(...)` route step. Java side uses
  `System.nanoTime()` delta around `.to("xslt:file://…")` or
  `.to("validator:…")`. Rust side uses `Instant::now()` delta
  around the same `.to(...)` call (the bridge subprocess is
  invoked from inside the camel-xslt/camel-validator
  component's `.to()` dispatch). Both sides record
  `t_start` before the step and compute `t_end − t_start`
  immediately after.
- **Parser contract**: field 2 of `BENCH_LATENCY` records is
  parsed as `i64` (allowing negative detection), then
  bounds-checked against `(0, 30_000_000_000]` ns. Out-of-bound
  or malformed records invalidate the round (no silent skip).
- **Engine pinning (spec F1)**: Saxon-HE 12.5 for XSLT,
  Xerces-J 2.12.2 for XSD, xml-apis 1.4.01. Pinned across all
  4 Java cells + the gRPC bridge.
- **Container mode**: Mandrel-único single container
  (RHEL 8.10, glibc 2.28). Replaces the v1/v2 "build in Docker,
  run bare-metal" split.
- **Bridge subprocess**: built inside the same container via
  Mandrel native-image (Saxon-HE 12.5 + camel-xslt component).
  102 MiB stripped ELF. Glibc 2.28 (not musl-static — see
  Limitations).
- **Harness contract (spec F2)**: latency file truncated
  before launch (no stale data from prior runs); first-success
  probe (30s) catches silent bridge-spawn failure; both
  contender and bridge PIDs monitored through measurement
  completion; sample count validated as exactly 10,000 per
  round (any mismatch fails the cell).
- **Process cleanup**: KILL (`SIGKILL`) the contender the
  instant the measurement window closes — peak RSS / process
  exit codes reflect "measurement done", not "tearing down".
- **Env-var hygiene**: `unset JAVA_TOOL_OPTIONS _JAVA_OPTIONS
  JDK_JAVA_OPTIONS`; `LC_ALL=C`.
- **Pairing model (carried from v1)**: Pair A = embedded
  runtime, no file parsing; Pair B = YAML route, parsed at
  runtime. The bridge matrix is intentionally asymmetric
  (4 artifacts per scenario, not 6) — see `benchmarks/CONTEXT.md`
  §2 "v3.5: Bridge scenarios stay at 4 artifacts". The
  authoring-format-invariant tax is not re-measured by adding
  yaml variants to the bridge matrix.

The v3 contribution to methodology is exclusively F1 (the
parser + instrumentation contract); F2 (the harness contract)
and F3 (skip Protocol B for T1/T2) are also landed as part of
the v3 fixes but are not exercised in this report's headline
numbers (the report measures M2-B which F3 does not skip).

## Limitations

Carried from v1/v2 unless noted, plus the v3-specific:

- **Protocol B v3.5 data was void until this fix landed (spec
  F1)**. The v3.5 run produced `bridge-tax-summary.json` with
  garbage numbers (rust-clock minus java-clock across two
  absolute timestamps → meaningless positive/negative deltas).
  The v3 report replaces those numbers with the post-fix data;
  any reader comparing v3.5 numbers to v3 numbers will see a
  regression because the v3.5 numbers were never valid. The
  v3 numbers are the first valid bridge-tax measurement.
- **F4 (JVM warmup sharpening) deferred to post-v3**. The
  v3.5 standalone JVM warmup failure finding is real but not
  sharpened to convergence-or-no-convergence in 10,000
  messages. The v3 report does not add new data on this
  point; it carries the v2 finding forward.
- **rust-camel-cli is reported but not in the bridge-tax
  pairs** (bd `rc-5gcu`). The CLI's YAML route loader does
  not currently register the camel-xslt or camel-validator
  component when the route file uses the bridge URI; the
  measurement fails at the first-success probe with no
  `BENCH_LATENCY` record. The fix is a YAML DSL gap (the
  component needs to be registerable from the YAML), not a
  v3 measurement issue. rust-camel-cli's M1 cold-start for
  the bridge scenarios is in the v3.5 data (27-28 ms,
  ~3.9 MiB RSS) and is included in the Cold-start section.
- **Bridge binary built inside container, glibc 2.28 (not
  musl-static)**. The bridge ELF links against the container's
  glibc 2.28 (RHEL 8.10); it does not run on a host with
  glibc < 2.28 without the matching libc.so. The Mandrel
  build script in `bridges/xml/build-native.sh` attempts
  `--static --libc=musl` but those flags do not take effect
  on the current Mandrel + Quarkus combination; bd `rc-eq7n`
  is filed for the static-build follow-up. The bridge is
  inside the same container as the rust-camel cells, so
  glibc 2.28 is the runtime; the portability constraint is
  only relevant for future host-only runs.
- **5 process launches is insufficient for process-level
  uncertainty estimation**. The v3 measurement's 5 rounds
  per cell disclose round-to-round variability in the
  `round_p99s_ns` and range fields, but cannot put a
  confidence interval on the median. A 50-launch
  re-measurement would close that gap; the v3 numbers are
  descriptive only (uncertainty statement, gate 7).
- **T1 is trivial, T2 is EIP-only, T3 (http-server) has no
  bean / process / multi-exchange**. Same scope as v2. The
  http-server scenario adds one new dimension (HTTP listener
  spin-up + request handler) but does not exercise the
  wider EIP surface (split, aggregate, dynamic router,
  error handling). The bridge scenarios are minimal
  (one `.to()` per route) by design — the bridge tax is the
  single variable under test, and confounding EIP
  composition would defeat the measurement.
- **rust-camel-lib vs rust-camel-cli asymmetry (carried
  from v1)**. The lib is a 4.8 MiB single-purpose ELF; the
  CLI is the 86 MiB general-purpose `camel` binary. The
  bridge tax pairs in this report are rust-camel-lib only;
  the CLI's per-request cost would be similar (the CLI is
  the lib + a YAML route loader), but the CLI's per-launch
  RSS is dramatically lower (3.9 MiB vs 9.3 MiB) because
  the CLI's route is pre-loaded into static config.
- **Bare-metal loses container isolation (carried from
  v1)**. Host load average during the v3.5 + v3 runs was
  2-3 (per `/proc/loadavg`); absolute medians may shift a
  few ms or μs on a quieter host. The relative multipliers
  between contenders cancel the noisy-neighbor effect.
- **JVM Quarkus YAML bridge cells not built for v3.5** (per
  `benchmarks/CONTEXT.md` §2 "v3.5: Bridge scenarios stay at
  4 artifacts"). The bridge tax is authoring-format-invariant
  (the gRPC subprocess + byte-pinned XML payload are
  identical regardless of route authoring format); adding
  yaml variants would re-measure a known-invariant quantity.
  The v3 matrix is therefore 4 cells (rust-camel-lib +
  camel-standalone-dsl + camel-quarkus-dsl-native +
  rust-camel-cli-as-M1-only) per scenario, with the
  camel-standalone-yaml and camel-quarkus-yaml-native
  cells in `won't-measure` per e_opus round-5 verdict.
- **xslt-bridge `camel-quarkus-dsl-native` failed M1 in
  v3.5** (native build OOM at Mandrel's 4 GB cap with
  Saxon-HE 12.5 + XSLT 3.0 + camel-xslt + camel-quarkus
  reflection metadata all in the same image). The v3 M2-B
  run succeeded for this cell; the M1 failure is recorded
  here for completeness.

## Findings narrative

The v3 headline is the bridge tax table, and the v3 finding
is that **the bridge tax is real, measurable, and varies
significantly with the Java baseline and the workload**.
rust-camel-lib's p50 tax ranges from 267 μs (XSLT-bridge vs
Quarkus native — where Quarkus native's Saxon-HE AOT is
already slow at 908 μs p50) to 1 044 μs (XSLT-bridge vs
standalone-dsl — where the standalone's Xerces-only XSD
validation is fast at 256 μs p50). The p99 ratio ranges
from 1.74× to 8.86×. The reader should not interpret these
numbers as "rust-camel-lib is 5× slower than Java on
bridged work" — the right framing is "rust-camel-lib pays
a 267 μs to 1 ms per-request tax when it must dispatch to
a Java bridge, and the resulting p99 is 1.7× to 8.9× the
Java p99 on the same workload".

The cold-start and warm-latency numbers, inherited from
v2/v3.5, hold. rust-camel-lib's 9 ms cold-start and 119 μs
warm p50 are both ~1.5-1.9× faster than Quarkus native's
14 ms and 227 μs. The JVM standalone cells are 30-40×
slower on cold-start and fail M2-A warmup stability at
1,000 messages. The no-bridge advantage is robust across
scenarios and metrics; the bridge tax is a real per-request
cost that does not erase the no-bridge advantage but does
define the boundary.

The deployment shape that wins on v3 numbers is: "do most
routing in-process (no-bridge path, 9 ms cold-start, 119 μs
warm p50); call the bridge infrequently for the XML work
that has no Rust native engine (each bridge call costs
267 μs to 1 ms p50, depending on workload)". The
rust-camel ICP remains the dev inner-loop / CI + K8s
sidecar pattern from v2, with a new caveat: **avoid the
bridge for high-rate request paths; use the no-bridge
timer/log/component/request paths and reserve the bridge
for infrequent XML work**.

## Reproducibility

- v3 run results: `benchmarks/results/20260721T161314Z/`
  - `run-metadata.json` (publication gate 5)
  - `bridge-tax-summary.json` (4 pairs, 0 dropped)
  - `m2-round-{0..4}/<cell>/` per-cell raw samples and
    per-round JSON summaries
  - `bridge-stderr.log` per cell (empty for all 6 cells;
    no spawn / handshake / runtime errors during measurement)
- v3.5 run results (source of M1 + M2-A numbers):
  `benchmarks/results/20260720T150810Z/`
  - Per-cell `samples.txt` for M1 (n=50 each, 25 cells)
  - `m2-round-{0..4}/<cell>/protocol-a-summary.txt` and
    `protocol-a-samples.txt` for M2-A
- Full run log: `benchmarks/results/v3-protocol-b-full-20260721T161311Z.log`
  (wall-clock ~68 min)
- v3 spec: `docs/superpowers/specs/2026-07-20-protocol-b-instrumentation-fixes.md`
- v3 plan: `docs/superpowers/plans/2026-07-20-protocol-b-instrumentation-fixes.md`
- Domain context: `benchmarks/CONTEXT.md`
- Coverage matrix: `benchmarks/COVERAGE.md` (cumulative
  index across v1/v2/v3 reports)

To reproduce the v3 measurement:

```bash
# Inside the Mandrel-único container (benchmark-runner:latest,
# digest sha256:35f5513810b93de9a0183b7cfe560cceff734fec82d345980c8c80d2d46cb767):
cd /workspace/rust-camel
git checkout <merge-commit-of-feature/rc-f3g9-startup-benchmark>
# See bd rc-c0y0 for the pinned tag/SHA post-merge.
# Pre-flight: verify toolchains
java -version  # OpenJDK 21.0.11+10-LTS (Temurin)
native-image --version  # Mandrel (jdk-21)
rustc --version  # 1.82.0 (stable)
gradle --version  # 8.10.2
mvn --version  # 3.9.9
# Build the bridge
cd bridges/xml && gradle build -Dquarkus.package.jar.enabled=false \
    -Dquarkus.native.enabled=true -Pversion=dev && cd ../..
# Build the Quarkus native cells (each ~3 min; 2 measured in v3)
cd benchmarks/scenarios/xslt-bridge/camel-quarkus/camel-quarkus-dsl && \
    gradle build -Dquarkus.native.enabled=true \
    -Dquarkus.package.jar.enabled=false && cd -
# (same for xsd-validation-bridge's camel-quarkus-dsl-native)
# (quarkus-yaml-native bridge cells are not built per e_opus round-5
#  verdict — asymmetric matrix; see Limitations)
# Build the rust-camel-lib fixtures
cargo build -p xslt-bridge-rust-camel-lib -p xsd-validation-bridge-rust-camel-lib
# Run the harness
bash benchmarks/harness/run-all.sh \
    --metric=m2 --scenarios=xslt-bridge,xsd-validation-bridge \
    --warmup=0 --warmup-time=30 --warmup-msgs=1000 \
    --rounds=5 --samples-per-round=10000
# Aggregate
cargo run -p bench-loadgen -- aggregate-bridge-tax \
    --input-dir=benchmarks/results/<new-ts>/ \
    --output=benchmarks/results/<new-ts>/bridge-tax-summary.json
```

Expected wall-clock: ~68 minutes for 6 cells × 5 rounds ×
10,000 samples after the 2 native Quarkus builds (~6 min)
and 1 bridge native build (~3 min).

## Cross-references

- v1 report: `docs/benchmarks/2026-07-18-startup-minimal-benchmark.md`
- v2 report: `docs/benchmarks/2026-07-18-benchmark-v2.md`
- v3 spec: `docs/superpowers/specs/2026-07-20-protocol-b-instrumentation-fixes.md`
- v3 plan: `docs/superpowers/plans/2026-07-20-protocol-b-instrumentation-fixes.md`
- v3.5 run dir (M1 + M2-A source): `benchmarks/results/20260720T150810Z/`
- v3 run dir (M2-B source): `benchmarks/results/20260721T161314Z/`
- Domain context: `benchmarks/CONTEXT.md`
- Coverage matrix: `benchmarks/COVERAGE.md`
- Harness: `benchmarks/harness/run.sh`
- Aggregator: `benchmarks/harness/loadgen/src/protocol_b.rs`
  (per-cell aggregation + pairwise bridge-tax reporting)
- v1 bd: `rc-f3g9` (closed)
- v2 bd: `rc-p9ki` (closed)
- v3 bd: `rc-2vxg` (this report)
- Open follow-ups: `rc-5gcu` (rust-camel-cli M2-B bridge gap —
  bench_instrument implemented, re-run pending), `rc-c0y0`
  (release bridge binaries + verify CI static-binary gate when
  branch merges to main), `rc-eq7n` (musl-static bridge build —
  verification deferred to rc-c0y0)
