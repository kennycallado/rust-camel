# Quarkus Native Throughput Diagnosis

> Analysis of why camel-quarkus-dsl-native and camel-quarkus-yaml-native
> show lower throughput than their JVM counterparts in M3 sustained
> throughput measurement. Empirically tested by e_opus (platform-http
> swap: +18.8% confirmed; Serial GC A/B: falsified); see
> `docs/benchmarks/e_opus-quarkus-native-and-bridge-analysis.md` for the
> pre-experiment hypothesis and action plan (note: the predicted
> 1.2–1.5× gap closure was NOT achieved — residual is 1.77×).

## Two contributing factors (e_opus empirical analysis)

1. **HTTP component — Jetty bypasses Quarkus' Vert.x/Netty layer (CONFIRMED,
   +18.8% resolved)**: The native fixtures used raw `camel-jetty:4.8.0`
   (the plain Apache Camel Jetty component), route URI
   `jetty:http://0.0.0.0:8080/bench`. This is *not* `camel-quarkus-http` and
   *not* platform-http — Quarkus' managed Vert.x/Netty HTTP layer is bypassed
   entirely. Swapping to `camel-quarkus-platform-http` (Quarkus' recommended
   HTTP component, backed by `camel-platform-http-vertx`) gives **+18.8%**:
   jetty native 32,770 → platform-http native 38,935 req/s. The swap is now
   applied to both native fixtures. (The JVM dsl sibling keeps `jetty:` so it
   stays comparable to camel-standalone-dsl, which is also jetty.)

2. **AOT vs JIT — the remaining gap is inherent to native-image (OPEN)**:
   Even with platform-http, the native runner is ~63% of the JVM baseline, so
   the swap does NOT close the full gap. Substrate VM compiles ahead-of-time
   at a fixed optimization level (`-O2`); it has no JIT to speculatively
   optimize the hot request path as the JVM does. Profile-guided optimization
   (PGO) is the documented path to narrow this: feed a representative run's
   profile back into `native-image` so the AOT compiler optimizes the
   measured hot path. Higher `-O3` is a lighter alternative. Neither is
   configured today; both are future build-work, not runtime work.

## Previously-suspected factors (now falsified or resolved)

- **Serial GC (FALSIFIED)**: Mandrel Community Edition defaults to Serial GC,
  which was the primary suspect (single-threaded collector, long pauses under
  allocation pressure). e_opus Experiment 2 measured camel-standalone-dsl
  (JVM) under both default G1GC and `-XX:+UseSerialGC` back-to-back: Serial GC
  throughput stayed ~63k req/s, indistinguishable from the G1 baseline. Serial
  GC is therefore NOT the cause of the native deficit. The `-R:MaxHeapSize=512m`
  cap is retained (it halves RSS without harming throughput).

- **Per-request stdout overhead (RESOLVED)**: Original fixtures emitted
  `System.out.println` on every request. Removed in commit 003523d7 as part of
  the "fair route" cleanup. No longer applies.

## Caveats

- The platform-http swap (+22% in full M3, +18.8% in spike test) is
  measured on the benchmark container with CPU pinning, 0 errors. Real,
  clean, reproducible.
- The residual gap (~56% of JVM throughput) is **attributed to
  AOT-vs-JIT but is an unproven hypothesis** — no flamegraph or PGO
  experiment has been run to confirm it. `perf` is not available in the
  current container image; profiling requires installing the tool and
  running a capture under M3 load. This remains the single open
  measurement gap in the v4 report.
- PGO is GraalVM EE-only — tested on both Mandrel CE 23.1.11 and
  GraalVM CE 21.0.2; neither supports `--pgo-instrument`. This lever
  cannot be exercised without a GraalVM EE license.
- GraalVM CE ships G1 for native-image (Mandrel CE does not), but this
  has not been tested in the benchmark. Serial GC was falsified as a
  throughput cause via JVM-side A/B (JVM-Serial ≈ JVM-G1), so switching
  to G1 on native is unlikely to help — but it remains an untested lever.
