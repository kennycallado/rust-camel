# Comparative benchmarks (bd rc-f3g9)

Extensible harness comparing rust-camel against Apache Camel across
**6 artifacts in 2 fair pairings** (each JVM fixture splits into a Pair A
"embedded runtime" and a Pair B "YAML-authored route"). **Not third-party
citations** — the earlier investigation (rc-f3g9 notes) found no citable,
Camel-specific official/third-party benchmark, so all numbers here are
measured by us.

## Quick start (container-hosted, v3.5)

Host requirements: **docker only** (Linux). No JDK, cargo, or Maven installation needed.

**Note**: requires Linux docker (`--network host` is a no-op on Docker Desktop). For macOS/Windows, out of scope for v3.5 — use a Linux VM or CI runner.

```bash
# Run the full v3 matrix (26 cells, ~5-6h wall-clock):
bash benchmarks/harness/run-all.sh \
    --metric=m1+m2 \
    --scenarios=startup-minimal,t2-realistic-eip,http-server,xslt-bridge,xsd-validation-bridge \
    --n=50 --warmup=3 --warmup-time=30 --warmup-msgs=1000 \
    --rounds=5 --samples-per-round=10000
```

The first run builds the `benchmark-runner:latest` image (~3-5 min one-time). Subsequent runs hit the docker build cache.

### What runs inside the container

- All cargo builds (rust-camel-lib × 5 + camel CLI + loadgen + bridge binary)
- All Maven builds (camel-standalone-{dsl,yaml} jars)
- All Gradle native builds (camel-quarkus-{dsl,yaml}-native — uses local native-image, no docker-in-docker)
- All cell measurements (GNU `time -v` wrapper, marker polling, Protocol A/B loadgen)

### Outputs

Results land at `benchmarks/results/<UTC-timestamp>/`:
- `m1-summary.csv` — cold-start medians + p95
- `m2-summary.csv` — warm p99 with BCa CI
- `<scenario>_<contender>/` — per-cell raw samples + RSS + Protocol A/B logs

### Cache volumes

Three named volumes persist across runs:
- `cargo-cache` — Rust crate registry
- `maven-cache` — Maven dep cache
- `gradle-cache` — Gradle dep + wrapper cache

To reset caches (e.g. after a dependency bump): `docker volume rm cargo-cache maven-cache gradle-cache`.

### Running on host directly (without container)

If you have JDK 21 + cargo + Maven installed locally, the legacy host mode still works:

```bash
export JAVA_HOME=/path/to/jdk21
bash benchmarks/harness/run.sh [args...]
```

NixOS-specific: also set `QUARKUS_NATIVE_CONTAINER_RUNTIME=local` and ensure `$JAVA_HOME/bin/native-image` is callable (requires GraalVM CE 21.0.2). The harness auto-detects this.

## Design: scenario-based, not a single fixed case

`timer -> log` (cold-start only) is the FIRST scenario, not the ONLY one.
rust-camel's differentiators go beyond cold-start (deterministic memory /
no-GC latency, throughput under sustained load, RSS under load) — the
harness must support adding scenarios without restructuring.

```
benchmarks/
├── README.md
├── harness/                                  # orchestration, shared across scenarios
│   └── run.sh                                # `./run.sh <scenario> [runs] [warmup-runs]`
└── scenarios/
    └── startup-minimal/                      # scenario 1: cold-start (timer -> log)
        ├── camel-standalone/                 # Maven multi-module (maven:3.9-eclipse-temurin-21)
        │   ├── pom.xml                       # parent, packaging=pom
        │   ├── camel-standalone-dsl/         # Pair A: hardcoded Java-DSL, NO yaml-dsl on classpath
        │   └── camel-standalone-yaml/        # Pair B: routes.yaml + camel-yaml-dsl
        ├── camel-quarkus/                    # Gradle multi-project (bridges/ Docker image)
        │   ├── settings.gradle.kts
        │   ├── camel-quarkus-dsl/            # Pair A
        │   └── camel-quarkus-yaml/           # Pair B
        └── rust-camel-lib/                   # Pair A: embedded library, no CLI/YAML
    # Plus, outside scenarios/, the rust-camel CLI+YAML contender:
    #   examples/camel-cli-run/routes/startup-minimal.yaml  (Pair B)
    #
    # future scenarios (not yet built, just the extension point):
    # ├── throughput-sustained/    # e.g. direct->filter->transform->log, msgs/sec
    # ├── memory-under-load/       # RSS over time under constant traffic (GC pause visibility)
    # └── http-echo/               # platform-http -> log, request/response latency p99
```

## Contenders — two fair pairings, not one flat comparison

Comparing Camel's hardcoded Java-DSL route against rust-camel's CLI+YAML
contender would mix two different questions ("which runtime is faster"
vs "what does the YAML authoring layer cost"). Each JVM fixture ships
**two separate build artifacts** (Maven modules / Gradle subprojects, per
oracle e_gpt Fix 4 — a shared jar/classpath would let Camel Main's
auto-discovery silently load Pair B's `routes.yaml` from Pair A's
classpath, invalidating the comparison) so both pairings are
apples-to-apples:

1. **Pair A — embedded runtime, no file parsing:**
   - `camel-standalone/camel-standalone-dsl/` (`App.java`, hardcoded Java-DSL)
   - `camel-quarkus/camel-quarkus-dsl/` (hardcoded Java-DSL `BenchRoute.java`)
   - `rust-camel-lib/` (programmatic route via public `CamelContext` API,
     no CLI/YAML — requires public lifecycle API or STOP, per Fix 5)
2. **Pair B — YAML-authored route, parsed at runtime:**
   - `camel-standalone/camel-standalone-yaml/` (`AppYaml.java` loads
     `routes.yaml` via `camel-yaml-dsl`)
   - `camel-quarkus/camel-quarkus-yaml/` (loads `routes.yaml` via
     `camel-quarkus-yaml-dsl`; runtime parsing verified per Fix 9)
   - rust-camel CLI + YAML (`camel run` +
     `examples/camel-cli-run/routes/startup-minimal.yaml`)

Full implementation plan: `docs/superpowers/plans/2026-07-18-rc-f3g9-startup-benchmark.md`.

## Scenario 1 — `startup-minimal` (cold-start / RSS / binary size)

Route: `timer:bench?repeatCount=1&delay=0` -> log `BENCH_ROUTE_READY`,
fires once immediately (`delay=0` per Fix 3 — Camel's default 1000ms
initial delay would otherwise silently inflate the measured startup by
~1s). Process is KILLed by the harness immediately after the marker so
peak RSS is recorded before any teardown allocations (Fix 7).

## Methodology (applies to every scenario)

- **3 warmup runs discarded** per contender (page-cache conditioning);
  warmup failures are FATAL — a failed warmup means the smoke test was
  lying and measured runs would silently drop data (Fix 1).
- **n=50 measured runs** per contender (Fix 8 — n=20 makes p95 = only the
  2nd-highest observation, too noisy for a public report). Report
  **median + nearest-rank p95** (`ceil(0.95*n)`, the standard for
  small-sample percentiles).
- **Randomized contender order** per run — avoids systematic thermal/cache
  bias favoring whichever contender always runs first/last.
- **Build in Docker, run bare-metal on host** (final arbitrated design,
  see plan doc "Measurement design correction" for the full 4-round
  e_opus/e_gpt arbitration history): the harness times from immediately
  before launching each artifact directly to the `BENCH_ROUTE_READY`
  marker — a single wall-clock, capturing full JVM/process bootstrap
  without container spin-up overhead. **No self-instrumentation** in any
  fixture (Fix 1/4-round history: self-instrumenting from inside `main()`
  skips JVM bootstrap, biasing toward JVM contenders).
- **Polling 1ms + wall-clock deadline** (Fix 2 — 10ms polling quantizes
  fast Rust results).
- **RSS**: `Maximum resident set size` from `/usr/bin/time -v` (peak, not
  an instantaneous sample that can land in a GC valley). Contender is
  KILLed immediately after the marker (Fix 7) so peak excludes
  graceful-shutdown teardown allocations.
- **Env-var hygiene** (Fix 6): `unset JAVA_TOOL_OPTIONS _JAVA_OPTIONS
  JDK_JAVA_OPTIONS` so they can't silently add/remove GC flags or
  agents; `LC_ALL=C` so `/usr/bin/time -v`'s "Maximum resident set size"
  label is in the form the harness greps for.
- **Process lifecycle** (Fix 1): harness resolves each contender's argv,
  launches `/usr/bin/time -v <argv...>`, reads the actual child PID
  (the `java`/`rust` process — NOT the `time` wrapper) from
  `/proc/$time_pid/task/$time_pid/children`, signals the child on
  cleanup, and `wait`s `time_pid` on every exit path. Cleanup trap
  covers Ctrl-C / SIGTERM mid-run.
- **Binary/artifact size**: full deployable directory for Quarkus
  (`du -sh quarkus-app/` — `quarkus-run.jar` is a thin launcher
  referencing `lib/` and `quarkus/` subdirs), single-file `du -h` for
  the Camel standalone jars-with-dependencies and the Rust binaries.
- **Report environment**: CPU model, kernel, container runtime, JDK/rustc
  versions, release profile flags — every scenario report repeats this
  (environment can drift between measurement sessions).

## Output

`docs/benchmarks/<date>-<scenario>-benchmark.md` — one report per
scenario, **two paired tables** (Pair A + Pair B), environment section,
and an explicit "not directly comparable" caveat for any measurement
asymmetry (e.g. bare-metal JVM vs bare-metal Rust binary, or rust-camel
CLI having no `delay=0` equivalent if the timer fires immediately by
default).

ICP naming: deferred until `startup-minimal` numbers exist — see rc-f3g9
notes for rejected ("scale-to-zero/edge") and candidate ICPs (K8s deployment
density, deterministic p99 latency without GC, dev inner-loop/CI). Pair A
and Pair B numbers may motivate *different* ICPs — don't force one
narrative across both if the data doesn't support it. Future scenarios
(throughput, memory-under-load) may motivate yet another ICP — don't
force one narrative across all scenarios either.

## Status

Skeleton only for `startup-minimal` — Java/Camel/Rust fixture code and
`harness/run.sh` not yet written.

**Blessing history** (full detail in plan doc
"Measurement design correction"): 5 arbitration/blessing rounds with
e_opus + e_gpt. Round 1 (e_opus B1/C1) + Round 2 (e_gpt rejection) →
Round 3 arbitration (build Docker + run bare-metal + single clock + no
self-instrumentation). Round 4 (e_gpt BLESS-WITH-FIXES, 10 fixes) applied.
Round 5 (e_gpt re-bless BLESS-WITH-FIXES, 5 more fixes: Bash arithmetic,
trap separation, smoke helper bg-launch, root.jar inspection, no `| tail`
masking) applied. Round 6 (e_gpt re-bless BLESS-WITH-FIXES, 5 more fixes:
`smoke()` wait on early-exit, Task 3 Step 9b `restore_yaml`+`trap - EXIT`
before final rebuild, no `set -e` short-circuit + `clean build`, Task 7
full AGENTS.md quality gates + Rust fixture check + "6 artifacts"
wording, README status update) applied. Awaiting convergence confirmation
via task_id consultation with e_gpt before execution begins.
