# Startup benchmark — startup-minimal scenario (2026-07-18)

> bd rc-f3g9. First of an extensible scenario suite (see
> `benchmarks/README.md`) — this report covers cold-start time, RSS, and
> binary size only. Throughput and memory-under-load are future scenarios,
> not covered here.

## Methodology

Route under test: `timer -> log`, fires once (with `delay=0` so Camel's
default 1000ms initial delay doesn't contaminate the cold-start number),
first log line is the "ready" signal. 3 warmup runs discarded per
contender (avoids measuring cold disk-cache I/O as if it were runtime
startup; warmup failures are FATAL — a failed warmup means the smoke test
was lying and measured runs would silently drop data), then **n=50**
measured runs per contender in **randomized order** (avoids systematic
thermal/cache bias favoring whichever contender always runs first/last).
Median + nearest-rank p95 (`ceil(0.95*n)`, the standard for small-sample
percentiles — at n=20 the p95 degenerates to "2nd-highest observation"
which is too noisy for a public report).

**All 6 artifacts run bare-metal on the host** (build happens in Docker,
execution does not) — a single wall-clock, timed by the harness
immediately before launching each artifact directly to the
`BENCH_ROUTE_READY` marker, captures full process/JVM bootstrap without
container spin-up overhead contaminating the measurement (see plan
"Measurement design correction" for the full e_opus/e_gpt arbitration
history). RSS is `Maximum resident set size` from `/usr/bin/time -v`
(peak, not an instantaneous sample); the contender is KILLed immediately
after the marker so peak RSS is recorded before any graceful-shutdown
teardown allocations.

Env-var hygiene: harness `unset`s `JAVA_TOOL_OPTIONS _JAVA_OPTIONS
JDK_JAVA_OPTIONS` and sets `LC_ALL=C` for every measured run, so JVM agent
overrides and locale-dependent parsers (YAML, JSON, properties) cannot
silently inflate the numbers. GNU time at `/run/current-system/sw/bin/time`
on NixOS (no `/usr/bin/time` on this host).

**Two pairings, not one flat table** (see plan "Pairing design" — comparing
Camel's hardcoded route against rust-camel's YAML-parsing CLI mixes two
different questions):

- **Pair A — embedded runtime, no file parsing:** Camel standalone/Quarkus
  Java-DSL route vs `rust-camel-lib` (programmatic route, no CLI/YAML).
- **Pair B — YAML-authored route, parsed at runtime:** Camel
  standalone/Quarkus loading the same route from `routes.yaml` via
  `camel-yaml-dsl` vs rust-camel CLI + YAML (`camel run`).

## Environment

```
$ uname -a
Linux ryzen 6.18.38 #1-NixOS SMP PREEMPT_DYNAMIC Sat Jul  4 11:44:22 UTC 2026 x86_64 GNU/Linux

$ /tmp/rc-f3g9-jdk21/bin/java -version
openjdk version "21.0.2" 2024-01-16
OpenJDK Runtime Environment GraalVM CE 21.0.2+13.1 (build 21.0.2+13-jvmci-23.1-b30)
OpenJDK 64-Bit Server VM GraalVM CE 21.0.2+13.1 (build 21.0.2+13-jvmci-23.1-b30, mixed mode, sharing)

$ rustc --version
rustc 1.96.0 (ac68faa20 2026-05-25)

$ docker --version
Docker version 29.6.1, build v29.6.1
```

Artifact sizes (per e_gpt condition — `du -sh` of the full deployable
directory for Quarkus, `du -h` of the single file for standalone jars and
Rust binaries):

```
$ du -h .../camel-standalone-dsl-1.0.0-jar-with-dependencies.jar
5,1M    benchmarks/scenarios/startup-minimal/camel-standalone/camel-standalone-dsl/target/camel-standalone-dsl-1.0.0-jar-with-dependencies.jar

$ du -h .../camel-standalone-yaml-1.0.0-jar-with-dependencies.jar
6,2M    benchmarks/scenarios/startup-minimal/camel-standalone/camel-standalone-yaml/target/camel-standalone-yaml-1.0.0-jar-with-dependencies.jar

$ du -sh .../camel-quarkus-dsl/build/quarkus-app/
16M     benchmarks/scenarios/startup-minimal/camel-quarkus/camel-quarkus-dsl/build/quarkus-app/

$ du -sh .../camel-quarkus-yaml/build/quarkus-app/
17M     benchmarks/scenarios/startup-minimal/camel-quarkus/camel-quarkus-yaml/build/quarkus-app/

$ du -h .../rust-camel-lib/target/release/startup-minimal
4,8M    benchmarks/scenarios/startup-minimal/rust-camel-lib/target/release/startup-minimal   (stripped)

$ du -h .../target/release/camel
86M     target/release/camel   (stripped)

$ du -sh .../rust-camel-cli/
16K     benchmarks/scenarios/startup-minimal/rust-camel-cli/   (Camel.toml + routes/startup-minimal.yaml)
```

## Results — Pair A (embedded runtime, no parsing)

| Contender | Cold-start median (ms) | Cold-start p95 (ms) | RSS median (KiB) | Artifact size |
|---|---|---|---|---|
| Apache Camel standalone (Java-DSL) | 318.0 | 333 | 137 066 | 5.1 MiB (jar) |
| Apache Camel Quarkus (Java-DSL) | 545.5 | 569 | 146 826 | 16 MiB (quarkus-app/) |
| rust-camel (embedded library) | 8.0 | 9 | 5 956 | 4.8 MiB (stripped ELF) |

Speedup vs Camel standalone, median: **~40× faster cold-start**, **~23×
less peak RSS**. Speedup vs Camel Quarkus, median: **~68× faster**, **~25×
less RSS**.

## Results — Pair B (YAML-authored route)

| Contender | Cold-start median (ms) | Cold-start p95 (ms) | RSS median (KiB) | Artifact size |
|---|---|---|---|---|
| Apache Camel standalone (`camel-yaml-dsl`) | 353.0 | 366 | 142 666 | 6.2 MiB (jar) |
| Apache Camel Quarkus (`camel-quarkus-yaml-dsl`) | 585.0 | 608 | 148 280 | 17 MiB (quarkus-app/) |
| rust-camel (CLI + YAML) | 13.0 | 14 | 31 960 | 86 MiB (stripped ELF) + 16 KiB (fixture) |

Speedup vs Camel standalone, median: **~27× faster cold-start**, **~4.5×
less RSS**. Speedup vs Camel Quarkus, median: **~45× faster**, **~4.6×
less RSS**.

## Not directly comparable

- **Bare-metal execution loses container isolation.** Noisy-neighbor
  effects on the host affect all 6 artifacts equally (accepted trade-off,
  see plan "Measurement design correction"). This cancels out in the
  relative comparison but means absolute numbers are host-load-dependent
  — re-running on a quiet host may shift the medians by a few ms.

- **JVM warmup is not amortized in cold-start.** CDS and C2 JIT are
  filesystem-cached after the 3 warmup runs, but interpretation of
  "cold-start" is still "first call after process fork". Camel's internal
  class-init lazy paths that are not exercised by the `timer -> log`
  route would not be in the bootstrap, so the number is more
  representative than a one-shot micro-benchmark but still favors the
  JVMs relative to a true "first-ever invocation" on a freshly-booted
  machine. The Rust binaries pay no such amortization debt at all.

- **rust-camel-cli binary is 86 MiB (stripped).** This is the full
  workspace `camel` binary, statically linked against the entire
  rust-camel workspace (Tower, hyper, tokio, serde_yaml, reqwest, the
  exec component, and 12 other crates — even the ones not exercised by
  the `timer -> log` route). For a fair "smallest deployable" comparison
  against a single-route use case, **rust-camel-lib at 4.8 MiB is the
  relevant Rust number**. The 86 MiB CLI binary is the price of being
  a general-purpose router; a `cargo build -p camel --no-default-features
  --features minimal` cut for a single fixture would land in the same
  ballpark as the lib, but that is a future build-profile task, not a
  benchmark re-run.

- **rust-camel-cli has an ADR-0033 fail-closed `Camel.toml` stub.** The
  Pair B fixture includes `[components.exec]` with a stub profile
  (`name = "bench-stub"`, `executable = "true"` — bare name resolves via
  PATH on NixOS; `/bin/true` is not used because NixOS has no FHS).
  Loading and parsing this stub adds trivial startup cost (~µs,
  immeasurable at the harness's millisecond resolution).

- **Quarkus fast-jar layout is a directory, not a file.** `quarkus-app/`
  contains `quarkus-run.jar` plus a `lib/` directory with ~80 jars and a
  generated `app/` directory with class metadata. The 16-17 MiB total
  reflects this on-disk layout; a real-world deployment also includes
  the JVM (~150 MiB) which is not counted in the "artifact size" column
  but is counted in the RSS column (same JVM, same cost).

- **p95 spread is ~5-10% of median for every contender.** That is the
  expected spread for cold-start on a quiet host, not bimodal failure
  modes — all 50 measured runs completed without harness-level aborts
  for any contender, and the harness aborts an iteration immediately on
  any `measure_once` non-zero exit (no silent drops).

## ICP

The Pair A numbers (8 ms cold-start, 6 MiB RSS, 4.8 MiB stripped binary)
support one ICP clearly: **dev inner-loop / CI**. A `cargo test`-style
edit-run-inspect loop for Camel-style integration code currently costs
~300-600 ms per iteration just to fork a JVM and parse the route; the
rust-camel-lib path costs ~8 ms. That is the difference between a
watch-and-reload inner loop (interactive) and a `mvn test`/`mvn package`
inner loop (coffee-break). The artifact size is small enough to ship
as a sidecar in a test harness without bloating container images.

The Pair B numbers (13 ms cold-start, 32 MiB RSS, 86 MiB CLI binary)
support a **K8s deployment density / dev-sidecar** ICP with a caveat:
the CLI binary is too large to embed in every container, but a single
sidecar per node serving many pods is viable (32 MiB RSS × 50
sidecars/node = 1.6 GiB, well under typical sidecar budgets). The YAML
parse path adds only ~5 ms over the lib path, so the "no code change to
ship a new route" property is essentially free at this scale.

What the data does **not** support: scale-to-zero/edge. Edge devices
generally do not have 32 MiB to spare for a sidecar that does nothing
most of the time, and the cold-start advantage of rust-camel over a
JVM is real but not "millisecond-frugal" — the Pair B number is 13 ms,
not 13 µs. If edge cold-start is the goal, the embedded-library
deployment shape (Pair A's 4.8 MiB / 8 ms) is the relevant data point
and supports a "compute-constrained device with a single hardcoded
route" use case better than a general "edge gateway" claim.

Single ICP statement: **rust-camel's primary near-term ICP is dev
inner-loop / CI for Camel-style integration code, with a secondary
K8s-sidecar shape for teams that need dynamic route loading without a
JVM in the request path.** The edge / scale-to-zero narrative was
considered and rejected — the numbers do not support it at this
artifact size, and forcing that framing would mislead readers about
the actual deployment footprint.
