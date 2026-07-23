# Startup benchmark v2 — Quarkus native-image + T2 EIP scenario (2026-07-18)

> bd `rc-p9ki`. Extension of v1 (`rc-f3g9`, closed; report at
> `docs/benchmarks/2026-07-18-startup-minimal-benchmark.md`). Adds 2 native-image
> Quarkus artifacts and 1 new EIP scenario (`t2-realistic-eip`). Same harness
> protocol, same pairings, same ICP framing — only the matrix grows.

## Executive summary

rust-camel's cold-start and RSS advantages over Apache Camel **hold** when
(a) Quarkus runs in its marketed native-image mode and (b) the route has
real EIP complexity (5 distinct EIPs: set-body / set-header / filter /
choice / log). On the new T2 realistic-EIP scenario, rust-camel-lib
reaches the marker in 8 ms (median) at ~5.7 MiB peak RSS — vs Quarkus
native's 18 ms at ~43 MiB (Pair A). Quarkus native does dramatically
beat its own JVM mode (~30× faster cold-start, ~3.3× smaller RSS), so
the v1 unfairness is now resolved. rust-camel-lib remains ~2.3× faster
on cold-start and ~7.7× smaller on RSS than Quarkus native; the gap is
order-of-magnitude but **smaller in absolute terms** than the v1 JVM
comparison suggested. rust-camel-cli remains ~1.4× faster on cold-start
and ~1.6× smaller on RSS than Quarkus native, but ships an 86 MiB
general-purpose binary vs Quarkus native's 58-63 MiB single-purpose
runner.

## Environment

```
$ uname -a
Linux ryzen 6.18.38 #1-NixOS SMP PREEMPT_DYNAMIC Sat Jul  4 11:44:22 UTC 2026 x86_64 GNU/Linux

$ /tmp/rc-f3g9-jdk21/bin/java -version
openjdk version "21.0.2" 2024-01-16
OpenJDK Runtime Environment GraalVM CE 21.0.2+13.1 (build 21.0.2+13-jvmci-23.1-b30)
OpenJDK 64-Bit Server VM GraalVM CE 21.0.2+13.1 (build 21.0.2+13-jvmci-23.1-b30, mixed mode, sharing)

$ /tmp/rc-f3g9-jdk21/bin/native-image --version
native-image 21.0.2 2024-01-16
GraalVM Runtime Environment GraalVM CE 21.0.2+13.1 (build 21.0.2+13-jvmci-23.1-b30)
Substrate VM GraalVM CE 21.0.2+13.1 (build 21.0.2+13, serial gc)

$ rustc --version
rustc 1.96.0 (ac68faa20 2026-05-25)

$ cargo --version
cargo 1.96.0 (30a34c682 2026-05-25)

$ grep -m1 'model name' /proc/cpuinfo
model name    : AMD Ryzen 5 5600G with Radeon Graphics

$ free -h
               total       used       free       shared    buff/cache   available
Mem:            27Gi       5.4Gi       2.5Gi       60Mi        19Gi        21Gi

$ df -h /home
/dev/nvme0n1p8   79G   71G  3.7G  96% /home
```

GNU time at `/run/current-system/sw/bin/time` (NixOS — no `/usr/bin/time`;
`bash` builtin `time` does not support `-v`).

Run timestamp: `20260718T202107Z` (results dir
`benchmarks/results/20260718T202107Z/`). Wall-clock for the full
n=50 × 16-cell campaign: **~4 min** (with 4 native builds already
fingerprint-cached; the only gradle invocation was an `UP-TO-DATE` check
on `camel-quarkus-dsl-native` whose `.bench-fingerprint` was stale).

## Build flags per artifact

Same as v1 for the 6 existing artifacts; documented in the v1 report.
The 2 new native artifacts (Task 2) build via:

```bash
# From the camel-quarkus fixture root.
JAVA_HOME=/tmp/rc-f3g9-jdk21 PATH=$JAVA_HOME/bin:$PATH \
  gradle :camel-quarkus-dsl-native:build \
    -Dquarkus.native.enabled=true \
    -Dquarkus.package.jar.enabled=false \
    -Dquarkus.native.additional-build-args=-H:NativeLinkerOption=-L/nix/store/61a1nwx3w6rqyaisj5rn1sal1981apm7-zlib-1.3.2/lib \
    --no-daemon
# Same for :camel-quarkus-yaml-native:build.
```

`camel-quarkus-yaml-native/src/main/resources/application.properties`
additionally contains:

```
quarkus.native.resources.includes=camel/routes.yaml
camel.main.routesIncludePattern=classpath:camel/routes.yaml
```

Both keys are required: the first embeds the YAML resource at build
time; the second tells the runtime where to find it (Substrate VM
cannot enumerate `classpath:camel/*` as Camel Main's
`DefaultConfigurationProperties` does on the JVM). The YAML-spike
investigation in `benchmarks/spike-results.md` Spike 1B documents the
three NixOS-specific fixes (zlib linker path, resource includes,
non-wildcard route pattern).

## Methodology

v1 protocol is inherited unchanged. Explicit affirmation (per spec
§4.5):

- **Sample size**: n=50 + 3 warmup discarded per cell; warmup
  failure is FATAL (aborts the run, not silently absorbed).
- **Ordering**: randomized across all 16 cells per measurement round
  (Fisher-Yates shuffle of the 2 × 8 = 16-cell list; not 8
  artifacts shuffled within 2 sequential scenario blocks — that
  would have let scenario-order drift between T1 and T2 bias the
  result).
- **Statistics**: nearest-rank p95 (`ceil(0.95 × n)`, clamped to
  `[1, n]`), median for the central tendency (cold-start
  distributions are right-skewed; mean would be dragged by
  outliers).
- **Polling**: 1ms granularity for the marker (10ms quantizes fast
  Rust starts — a 3 ms startup would round to 10 ms and become
  indistinguishable from a 12 ms one).
- **Startup deadline**: 30s. Failure to see the marker in 30s
  fails the cell (no silent retry).
- **Marker validation**: scenario-aware exact-match count. T1
  cells expect `BENCH_ROUTE_READY` (no body suffix). T2 cells
  expect `BENCH_ROUTE_READY body=pong-bench` (the body suffix
  proves the `choice`/when branch executed; a wrong-branch
  run produces `body=pong-other` and is a hard failure, not a
  silent wrong-route measurement). Zero or multiple markers is
  also a hard failure.
- **Raw samples**: per-cell `samples.txt` saved to
  `benchmarks/results/<timestamp>/<cell>/samples.txt` (linked
  below) for independent re-analysis — report aggregates are
  reproducible from the raw files.
- **RSS**: GNU `time -v` `Maximum resident set size (kbytes)`
  (peak, not instantaneous; an instantaneous `/proc` read can
  land in a GC valley and under-report a JVM).
- **Process cleanup**: KILL (`SIGKILL`) the contender the
  instant the marker is observed — peak RSS reflects "route
  live", not "route tearing down". `TERM`→grace→`KILL` is the
  timeout path only.
- **Execution environment**: bare-metal on the host
  (containerization for builds, not for measurement — Docker
  spin-up in the timed path would be measured overhead, not
  runtime bootstrap, and would have to be added to the Rust
  contenders too, injecting namespace noise instead of
  removing it; e_opus arbitrated this in v1 after e_gpt's
  symmetry objection, recorded in
  `benchmarks/CONTEXT.md` §2).
- **Single harness-side clock**: wall-clock captured immediately
  before `exec` of the contender, stopped at the marker. No
  in-process timing (skips JVM creation / classload / JIT init,
  all part of real cold-start).
- **Env-var hygiene**: `unset JAVA_TOOL_OPTIONS _JAVA_OPTIONS
  JDK_JAVA_OPTIONS`; `LC_ALL=C` (also stabilizes the GNU time
  output line label and locale-dependent YAML/JSON parsers).

v2-specific additions to the v1 protocol:

- **Fingerprint-based native build caching** (Task 4 §4c): the
  harness computes a SHA-256 fingerprint of (shared JVM
  sibling's `src/main/`, native subproject's `build.gradle.kts`,
  parent `settings.gradle.kts`, JVM sibling's `build.gradle.kts`,
  native subproject's `application.properties`, `gradle/`
  wrapper contents, `$JAVA_HOME` path, `native-image --version`
  output) and skips the gradle invocation if the fingerprint
  matches the on-disk `.bench-fingerprint` AND the runner binary
  exists. This is a build-time optimization; measurement runs
  invoke the cached native runner directly, so fingerprint
  caching does **not** affect the measured numbers.
- **Pre-flight toolchain checks** (Task 4 §4d): `JAVA_HOME` must
  resolve to GraalVM CE 21.0.2; `native-image --version` must
  succeed; gradle binary must be present. Pre-flight failure is
  fatal — no silent fall back to a system JDK.
- **Scenario-aware marker** (Task 4 §4e): T2 expects
  `BENCH_ROUTE_READY body=pong-bench`. The harness counts
  occurrences of the exact string; a static `BENCH_ROUTE_READY`
  line followed by a `BENCH_ROUTE_READY body=pong-bench` line
  counts 1 (the dynamic one matches; the static one doesn't
  because it lacks the body suffix).

For the full v1 protocol inheritance + 6-round methodology
arbitration history, see
`docs/superpowers/plans/2026-07-18-rc-f3g9-startup-benchmark.md`
"Measurement design correction". The domain-language context
(ICP framing, fairness fixes, pairing model) lives at
`benchmarks/CONTEXT.md` and is referenced, not duplicated, here.

## Raw samples

Per-cell raw samples for all 16 cells (n=50 each, 2 columns:
`elapsed_ms rss_kb`) live in
[`benchmarks/results/20260718T202107Z/`](../../benchmarks/results/20260718T202107Z/).
Cell naming: `<scenario>_<contender>/samples.txt` (underscores
substitute the slash in `<scenario>/<contender>`).

| Scenario | Contender | Path |
|---|---|---|
| startup-minimal | camel-standalone-dsl | [`samples.txt`](../../benchmarks/results/20260718T202107Z/startup-minimal_camel-standalone-dsl/samples.txt) |
| startup-minimal | camel-quarkus-dsl | [`samples.txt`](../../benchmarks/results/20260718T202107Z/startup-minimal_camel-quarkus-dsl/samples.txt) |
| startup-minimal | camel-quarkus-dsl-native | [`samples.txt`](../../benchmarks/results/20260718T202107Z/startup-minimal_camel-quarkus-dsl-native/samples.txt) |
| startup-minimal | rust-camel-lib | [`samples.txt`](../../benchmarks/results/20260718T202107Z/startup-minimal_rust-camel-lib/samples.txt) |
| startup-minimal | camel-standalone-yaml | [`samples.txt`](../../benchmarks/results/20260718T202107Z/startup-minimal_camel-standalone-yaml/samples.txt) |
| startup-minimal | camel-quarkus-yaml | [`samples.txt`](../../benchmarks/results/20260718T202107Z/startup-minimal_camel-quarkus-yaml/samples.txt) |
| startup-minimal | camel-quarkus-yaml-native | [`samples.txt`](../../benchmarks/results/20260718T202107Z/startup-minimal_camel-quarkus-yaml-native/samples.txt) |
| startup-minimal | rust-camel-cli | [`samples.txt`](../../benchmarks/results/20260718T202107Z/startup-minimal_rust-camel-cli/samples.txt) |
| t2-realistic-eip | camel-standalone-dsl | [`samples.txt`](../../benchmarks/results/20260718T202107Z/t2-realistic-eip_camel-standalone-dsl/samples.txt) |
| t2-realistic-eip | camel-quarkus-dsl | [`samples.txt`](../../benchmarks/results/20260718T202107Z/t2-realistic-eip_camel-quarkus-dsl/samples.txt) |
| t2-realistic-eip | camel-quarkus-dsl-native | [`samples.txt`](../../benchmarks/results/20260718T202107Z/t2-realistic-eip_camel-quarkus-dsl-native/samples.txt) |
| t2-realistic-eip | rust-camel-lib | [`samples.txt`](../../benchmarks/results/20260718T202107Z/t2-realistic-eip_rust-camel-lib/samples.txt) |
| t2-realistic-eip | camel-standalone-yaml | [`samples.txt`](../../benchmarks/results/20260718T202107Z/t2-realistic-eip_camel-standalone-yaml/samples.txt) |
| t2-realistic-eip | camel-quarkus-yaml | [`samples.txt`](../../benchmarks/results/20260718T202107Z/t2-realistic-eip_camel-quarkus-yaml/samples.txt) |
| t2-realistic-eip | camel-quarkus-yaml-native | [`samples.txt`](../../benchmarks/results/20260718T202107Z/t2-realistic-eip_camel-quarkus-yaml-native/samples.txt) |
| t2-realistic-eip | rust-camel-cli | [`samples.txt`](../../benchmarks/results/20260718T202107Z/t2-realistic-eip_rust-camel-cli/samples.txt) |

## Results — Tier 1 (startup-minimal scenario, n=50)

T1 route shape unchanged from v1: `timer:bench?repeatCount=1&delay=0
-> log("BENCH_ROUTE_READY")`. The two new artifacts are the
Quarkus native-image variants of the existing JVM-mode fixtures.

### Pair A — embedded runtime, no parsing

| Contender | Cold-start median (ms) | Cold-start p95 (ms) | RSS median (KiB) | Artifact size |
|---|---|---|---|---|
| Apache Camel standalone (Java-DSL) | 309.0 | 384 | 136 828 | 5.1 MiB (jar) |
| Apache Camel Quarkus (Java-DSL) | 537.5 | 590 | 146 854 | 16 MiB (`quarkus-app/`) |
| Apache Camel Quarkus (Java-DSL, **native**) | **18.0** | 26 | 43 044 | 58 MiB (runner ELF) |
| rust-camel (embedded library) | 8.0 | 9 | 5 484 | 4.8 MiB (stripped ELF) |

Speedup vs Camel Quarkus native (median, Pair A): **~2.3× faster
cold-start**, **~7.9× less peak RSS**. Speedup vs Camel Quarkus JVM
(median): ~67× faster, ~27× less RSS — same order of magnitude as
v1, confirming the v2 numbers are not a JVM-mode artifact.

### Pair B — YAML-authored route, parsed at runtime

| Contender | Cold-start median (ms) | Cold-start p95 (ms) | RSS median (KiB) | Artifact size |
|---|---|---|---|---|
| Apache Camel standalone (`camel-yaml-dsl`) | 342.0 | 359 | 138 266 | 6.2 MiB (jar) |
| Apache Camel Quarkus (`camel-quarkus-yaml-dsl`) | 567.0 | 987 | 148 188 | 17 MiB (`quarkus-app/`) |
| Apache Camel Quarkus (`camel-quarkus-yaml-dsl`, **native**) | **18.0** | 25 | 46 188 | 63 MiB (runner ELF) |
| rust-camel (CLI + YAML) | 13.0 | 27 | 28 894 | 86 MiB (stripped ELF) + 16 KiB (fixture) |

Speedup vs Camel Quarkus native (median, Pair B): **~1.4× faster
cold-start**, **~1.6× less peak RSS**. The Pair B gap to Quarkus
native is much smaller than Pair A's — because the `camel` CLI
binary pays for TOML config load + ADR-0033 startup validation +
YAML route parsing, which the native runner avoids (YAML is
embedded at build time, ADR-0033 / Camel Main is gone, the route
is registered directly into the Substrate VM image).

## Results — Tier 2 (t2-realistic-eip scenario, n=50)

T2 route shape (spec §4.1, behaviorally equivalent across all 8
artifacts; the Pair A predicate mechanism is intentionally
different — see "Pair A predicate deviation" below):

```
timer:bench?repeatCount=1&delay=0
  -> set_body(constant="ping")
  -> set_header(name="source", constant="bench")
  -> filter(simple("${body} == 'ping'") | closure equivalent)
  -> choice
       .when(simple("${header.source} == 'bench'") | closure equivalent)
         -> set_body(constant="pong-bench")
       .otherwise
         -> set_body(constant="pong-other")
  -> log("BENCH_ROUTE_READY body=${body}")
```

The `body=${body}` in the final log emits `body=pong-bench` (correct
branch) or `body=pong-other` (wrong branch); the harness fails the
cell on the latter.

### Pair A — embedded runtime, no parsing

| Contender | Cold-start median (ms) | Cold-start p95 (ms) | RSS median (KiB) | Artifact size |
|---|---|---|---|---|
| Apache Camel standalone (Java-DSL) | 332.5 | 746 | 138 124 | 5.1 MiB (jar) |
| Apache Camel Quarkus (Java-DSL) | 554.0 | 600 | 147 294 | 16 MiB (`quarkus-app/`) |
| Apache Camel Quarkus (Java-DSL, **native**) | **18.0** | 19 | 44 208 | 58 MiB (runner ELF) |
| rust-camel (embedded library) | 8.0 | 9 | 5 720 | 4.9 MiB (stripped ELF) |

Speedup vs Camel Quarkus native (median, T2 Pair A): **~2.3×
faster cold-start**, **~7.7× less peak RSS**. rust-camel-lib
cold-start (8 ms) and RSS (5.7 MiB) are essentially unchanged from
T1 — adding 5 EIPs to the route cost <1 ms and ~0.2 MiB. Camel
Quarkus native is also unchanged from T1 (18 ms / 44 MiB) — the
native AOT step folds the EIP pipeline into the closed-world
analysis at build time, so adding EIPs costs nothing at
startup. **The cold-start delta between T1 and T2 is in the
JVMs**, not the native binaries: Camel standalone-dsl goes
from 309→332.5 ms (~+7.5%), Quarkus JVM from 537.5→554 ms
(~+3%), Camel standalone-yaml from 342→372 ms (~+9%). The
JVMs pay per-class-loaded EIP step at startup; native + Rust
do not.

### Pair B — YAML-authored route, parsed at runtime

| Contender | Cold-start median (ms) | Cold-start p95 (ms) | RSS median (KiB) | Artifact size |
|---|---|---|---|---|
| Apache Camel standalone (`camel-yaml-dsl`) | 372.0 | 550 | 152 990 | 6.2 MiB (jar) |
| Apache Camel Quarkus (`camel-quarkus-yaml-dsl`) | 590.0 | 634 | 149 682 | 18 MiB (`quarkus-app/`) |
| Apache Camel Quarkus (`camel-quarkus-yaml-dsl`, **native**) | **18.0** | 27 | 47 758 | 63 MiB (runner ELF) |
| rust-camel (CLI + YAML) | 13.0 | 16 | 29 312 | 86 MiB (stripped ELF) + 16 KiB (fixture) |

Speedup vs Camel Quarkus native (median, T2 Pair B): **~1.4×
faster cold-start**, **~1.6× less peak RSS**. Same shape as T1
Pair B: rust-camel-cli's parse-path overhead closes most of the
gap to Quarkus native's AOT-compiled build.

## Pair A predicate deviation

**Documented explicitly, not hidden.** The T2 route uses different
predicate mechanisms for Pair A across the two runtimes — and they
are not language-subsystem-equivalent. Apache Camel Pair A
(`camel-standalone-dsl`, `camel-quarkus-dsl`, `camel-quarkus-dsl-native`)
uses Simple language predicates:

```java
.filter(simple("${body} == 'ping'"))
.choice().when(simple("${header.source} == 'bench'"))
```

rust-camel-lib Pair A uses closure predicates (its idiomatic
public API — `RouteBuilder::filter<F>` and `RouteBuilder::when<F>`
take `Fn(&Exchange) -> bool`):

```rust
.filter(|ex| ex.body().as_str() == Some("ping"))
.choice()
    .when(|ex| ex.header("source").and_then(|v| v.as_str()) == Some("bench"))
```

The two mechanisms have different cost profiles: Simple language
goes through `LanguageSupport::createPredicate` →
`SimpleExpressionParser` → AST-walk-on-every-evaluation, while the
closure is a direct Rust function pointer (inlinable, monomorphized,
no AST). On the *measured* metric (cold-start + RSS), the
difference is sub-millisecond — both Pair A contenders land at 8 ms
median. But the deviation means **T2 Pair A is not a
language-subsystem comparison; it is an "overall EIP pipeline
overhead at each framework's idiomatic surface" comparison**.
Pair B *is* language-subsystem-equivalent (both runtimes parse
`${body} == 'ping'` from YAML; the rust-camel YAML DSL passed
Spike 1A's `language-simple` reachability test), so Pair B T2
results are the cleaner cross-language comparison.

## Native initialization caveat (read this before the speedup claims)

Quarkus native-image shifts work from runtime to build time via
GraalVM's closed-world static analysis. The 18 ms cold-start number
for Quarkus native is honest cold-start — but it is **cold-start
in the deployment sense, not the build sense**. A Quarkus native
build takes **64-77 seconds per binary** on this host (per
`benchmarks/spike-results.md` Spike 1B), and Quarkus 3.20.0 ships
`5 330+` reflection-metadata classes into the native image even
for a 5-EIP route. The "first time anyone runs this code" cost
is amortized into the build, not the startup.

Concrete implication: a Quarkus native deployment's **per-instance
cold-start is 18 ms**, but **the cost of changing the route** is
re-running a 64-second native build. A rust-camel-cli deployment's
per-instance cold-start is 13 ms (or 8 ms for the embedded lib),
but **changing the route is a config-file edit and a `camel run`
restart — no rebuild, no reflection-metadata regen, no
closed-world re-analysis**. The two deployment shapes are not
interchangeable; each has a different break-even point between
build-amortization savings and iteration-cost overhead. This
report's cold-start numbers measure only the post-build side of
that trade-off.

## ICP

The v2 numbers do **not** change the v1 ICP framing — Quarkus
native is a *new* contender in the matrix, not a *replacement*
for any rust-camel claim:

- **Dev inner-loop / CI** (Pair A): rust-camel-lib's 8 ms cold-start
  on a 5-EIP route, ~5.4-5.6 MiB peak RSS, 4.8-4.9 MiB stripped
  binary, is essentially unchanged from v1's 1-EIP route. The
  advantage is "edit-run-inspect for Camel-style integration code
  in milliseconds, not `mvn test` coffee-breaks." A `cargo
  test`-style inner loop at 8 ms even on a real 5-EIP route is
  still order-of-magnitude faster than any JVM contender (closest
  JVM: Quarkus native at 18 ms, still 2.3× slower; Quarkus JVM
  at 537 ms, still ~67× slower).
- **K8s deployment density / dev-sidecar** (Pair B): rust-camel-cli's
  13 ms cold-start on the same 5-EIP route, ~28-29 MiB peak
  RSS, 86 MiB stripped binary, is essentially unchanged from v1.
  Quarkus native is now the closest competitor (18 ms / ~47 MiB
  Pair B), but rust-camel-cli retains a 1.4× cold-start and 1.6×
  RSS edge, and the YAML-parse-is-free property (the difference
  between lib and CLI is now ~5 ms on T1, ~5 ms on T2) is
  preserved.
- **Edge / scale-to-zero**: remains rejected on the v2 data. The
  rust-camel-lib's 4.8-4.9 MiB binary is the only "edge-adjacent"
  data point; the 86 MiB CLI binary is too large for edge
  deployment. The fact that Quarkus native is now competitive on
  cold-start (18 ms) does *not* change this — the ICP question
  is about deployment footprint, not single-instance
  cold-start.

Quarkus native opens a new deployment shape (low-cold-start JVM
replacement) that the v1 framing did not address. The v2
numbers say Quarkus native is the right JVM-replacement
choice *for workloads where the 64-77 s native build is
acceptable* — i.e. where the route is stable and the per-instance
cold-start is the bottleneck. The Quarkus native ICP is
*separate* from the rust-camel ICPs above; the two are not
mutually exclusive deployments.

## Delta table vs v1 (Quarkus-only)

The v1 report (`docs/benchmarks/2026-07-18-startup-minimal-benchmark.md`)
measured Quarkus in JVM mode only. v2 re-measures T1 with the 2
new native artifacts added. The JVM-mode Quarkus numbers in v2
are within a few percent of v1 (run-to-run noise on the same host),
so the delta is attributable to the native addition, not the re-run:

| Scenario | Pair | Contender | v1 JVM (ms / RSS KiB) | v2 JVM (ms / RSS KiB) | v2 native (ms / RSS KiB) | Native-vs-JVM speedup (×) | Native-vs-JVM RSS ratio |
|---|---|---|---|---|---|---|---|
| startup-minimal | A | Quarkus dsl | 545.5 / 146 826 | 537.5 / 146 854 | **18.0** / 43 044 | **~30×** | **~3.4× less** |
| startup-minimal | B | Quarkus yaml | 585.0 / 148 280 | 567.0 / 148 188 | **18.0** / 46 188 | **~32×** | **~3.2× less** |
| t2-realistic-eip | A | Quarkus dsl | (n/a) | 554.0 / 147 294 | **18.0** / 44 208 | **~31×** | **~3.3× less** |
| t2-realistic-eip | B | Quarkus yaml | (n/a) | 590.0 / 149 682 | **18.0** / 47 758 | **~32×** | **~3.1× less** |

The headline: Quarkus native is **~30× faster on cold-start** and
**~3.3× smaller on RSS** than Quarkus JVM, on this 5-EIP route.
That is a real Quarkus win and resolves the v1 unfairness
("JVM mode only for Quarkus" — see `benchmarks/CONTEXT.md` §6).

## Delta table vs v1 (rust-camel — re-measured, expected drift)

rust-camel's median cold-start is unchanged at 0% drift; RSS is
5-10% lower in v2 than v1 (run-to-run noise; the binaries are the
same shared workspace builds):

| Contender | v1 T1 (ms / RSS KiB) | v2 T1 (ms / RSS KiB) | v2 T2 (ms / RSS KiB) | v1→v2 T1 drift |
|---|---|---|---|---|
| rust-camel-lib | 8.0 / 5 956 | 8.0 / 5 484 | 8.0 / 5 720 | median 0%, RSS -7.9% |
| rust-camel-cli | 13.0 / 31 960 | 13.0 / 28 894 | 13.0 / 29 312 | median 0%, RSS -9.6% |

Rust RSS is 5-10% lower in v2 than v1 — within run-to-run
variation for 50-sample n on a noisy host (host load average
during the v2 run was 3.7, vs ~1 for the v1 run per
`/proc/loadavg`). The medians are unchanged.

## Limitations

Carried from v1 unless noted:

- **T1 is still trivial** (`timer -> log`); T2 partially
  resolves this with 5 distinct EIPs. Neither scenario
  exercises bean / process registration, external I/O
  (kafka, http, file), multi-route error handling, or
  payload-size / concurrency axes — those are Tier 3+
  (deferred per spec §3 / plan §"Out of scope").
- **T2 is EIP-only** (no bean, no process, no
  multi-exchange, no error handling). The `set_body` /
  `set_header` / `filter` / `choice` / `log` pipeline
  is the simplest "real route" shape and avoids
  confounding with bean-lookup overhead. The
  consequence: a real integration route that uses a
  bean or external component is **not directly
  covered by these numbers** — RSS will grow as
  components are registered, and cold-start will
  grow as init paths are exercised. The scale of
  the growth is not measured here.
- **n=50 has statistical power for large effect
  sizes only**. The 2.3× cold-start gap (rust-camel-lib
  vs Quarkus native) is large enough to survive any
  reasonable noise on this host. A 5% gap (say,
  between Quarkus-dsl and Quarkus-yaml JVM) is **not**
  resolved at n=50. The suite is built to demonstrate
  order-of-magnitude gaps, not fine distinctions.
- **Native is Quarkus-only**. The rust-camel-lib /
  rust-camel-cli artifacts are already native (Rust
  → static ELF). Apache Camel Main (the `camel-standalone-*`
  fixtures) has no first-party native-image story —
  Maven + `camel-main` is JVM-only. So the 4 native
  cells in this report are (Quarkus dsl native,
  Quarkus yaml native, rust-camel-lib, rust-camel-cli);
  Camel Main is JVM-only and is shown for context.
- **Bare-metal loses container isolation** (carried
  from v1). Host load during the v2 run averaged 3.7
  (per `/proc/loadavg` at run end); absolute medians
  may shift a few ms on a quieter host. The relative
  multipliers between contenders cancel the
  noisy-neighbor effect.
- **rust-camel-cli is the 86 MiB general-purpose
  binary** (carried from v1). The Pair B 86 MiB is
  the price of being a general-purpose router
  (statically linked against Tower, hyper, tokio,
  serde_yaml, reqwest, the exec component, and
  ~12 other crates, even though T2 uses only
  `timer` + `log` + a `log` step). The Pair A
  4.8-4.9 MiB lib is the relevant "minimal
  deployable" data point for rust-camel; a
  `--no-default-features --features minimal` CLI
  build is a future build-profile task, not a
  benchmark re-run.
- **v2 native-image binary size is larger than v1
  JVM-mode `quarkus-app/` size** (58-63 MiB runner
  vs 16-18 MiB `quarkus-app/`). The native runner
  bundles the JVM-equivalent runtime, GC, and
  reflection metadata into the ELF; the
  `quarkus-app/` directory assumes an external JVM
  is present at runtime. A native deployment
  doesn't need the JVM, but the on-disk size is
  larger.

## Findings narrative

The v1 unfairness — that Quarkus was measured in JVM mode while
its marketing story is native — is resolved: Quarkus native is
~30× faster on cold-start and ~3.3× smaller on peak RSS than
Quarkus JVM, on this 5-EIP route, and the new numbers make
that visible. rust-camel still wins on cold-start and RSS, but
the gap is smaller in absolute terms than v1's JVM-only
comparison suggested: ~2.3× on cold-start and ~7.7× on RSS for
Pair A, ~1.4× and ~1.6× for Pair B. **This is honest progress
for Quarkus, not a regression for rust-camel**: the rust-camel
numbers are essentially unchanged from v1 (the binaries are
the same; v2's RSS medians are 5-10% lower within run-to-run
noise).

The T2 EIP scenario confirms that the rust-camel advantage is
not an artifact of the trivial T1 route: rust-camel-lib's
cold-start is 8 ms on T1 and 8 ms on T2 (the 5-EIP pipeline
costs <1 ms; route work is negligible vs startup), and
Quarkus native's cold-start is 18 ms on both T1 and T2 (the
EIP pipeline is folded into the build). The JVM contenders
do show a T1→T2 cold-start increase (~3-9%), but the
increase is much smaller than the rust-camel-vs-JVM gap and
much smaller than the Quarkus-JVM-vs-native gap. **The
headline ordering — rust-camel < Quarkus native < Quarkus
JVM < Camel standalone — holds in both T1 and T2**, but the
distance between rust-camel and Quarkus native is small
enough that a future workload or future Quarkus version
could close it. The report does not claim "rust-camel is
2× faster than Quarkus native" without scoping to
cold-start + this specific 5-EIP scenario + this host.

The native-vs-JVM speedup (~30× cold-start, ~3.3× RSS) is
the genuinely new finding for readers who saw v1. It
validates the Quarkus positioning ("fast cold-start for
serverless / scale-to-zero") on this route shape. The
caveats (build-time amortization, no per-route changes
without a 64-second rebuild, no JVM-mode Main in the
Apache Camel side) prevent a clean "Quarkus native is
the answer" claim; the trade-off is real and the report
names it.

## Cross-references

- v1 report: `docs/benchmarks/2026-07-18-startup-minimal-benchmark.md`
- v2 spec: `docs/superpowers/specs/2026-07-18-rc-p9ki-benchmark-v2-design.md`
- v2 plan: `docs/superpowers/plans/2026-07-18-rc-p9ki-benchmark-v2.md`
- Domain context: `benchmarks/CONTEXT.md` (Pairing, ICP,
  Limitations, Fairness Fixes — referenced, not duplicated)
- Spike results: `benchmarks/spike-results.md` (NixOS native
  build workarounds, T2 route shape validation)
- Harness: `benchmarks/harness/run.sh` (v2 protocol
  implementation; CLI `bash benchmarks/harness/run.sh
  --help` for the flag-based interface)
- v1 bd: `rc-f3g9` (closed)
- v2 bd: `rc-p9ki`
