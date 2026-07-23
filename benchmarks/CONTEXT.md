# CONTEXT — Benchmark Domain (rust-camel comparative benchmarks)

> Domain-level context for the rust-camel comparative benchmark suite (bd
> `rc-f3g9` Tier 1 and successors). Captures the domain language, methodology
> decisions, and open design space established while building the
> `startup-minimal` scenario. This is **reference** material, not a plan — it
> records WHAT the domain terms mean and WHY the methodology is the way it is,
> not WHAT to build next.
>
> **Authority note:** this doc is subordinate to the benchmark harness code
> (`benchmarks/harness/run.sh`, fixture sources) and the published report
> (`docs/benchmarks/2026-07-18-startup-minimal-benchmark.md`). Where prose and
> code disagree, code wins. Cross-references, not duplication: the plan
> (`docs/superpowers/plans/2026-07-18-rc-f3g9-startup-benchmark.md`) holds the
> full 6-round methodology arbitration history; this doc synthesizes the
> load-bearing conclusions.
>
> **Scope boundary:** "benchmark domain" here means the *comparative,
> published, ICP-anchored* suite under `benchmarks/`. It is distinct from the
> in-repo criterion micro-benchmarks at `crates/camel-bench/` (`pipeline.rs`,
> `throughput.rs`, `body_coercion.rs`, …), which measure internal regressions,
> not cross-runtime parity. Both are "benchmarks"; only the former is this
> domain.

---

## 1. Domain Language (glossary)

Definitions are built from how the project **actually** uses each term in the
harness, plan, and report — not generic dictionary senses.

| Term | Definition (project-specific) |
|---|---|
| **Suite** | The whole comparative benchmark effort under `benchmarks/`, versioned by bd issue (v1 = `rc-f3g9`). Distinct from `crates/camel-bench/` criterion harnesses (internal regression, not cross-runtime). |
| **Tier** | A cohort of scenarios grouped by workload complexity, not by metric. Tier 1 = trivial single-route cold-start (`timer -> log`). Higher tiers add EIPs, external deps, multi-route error handling. Tier is about *what the route does*; see also "metric" (orthogonal axis). |
| **Scenario** | One named workload with a fixed route topology and a fixed "ready" signal, buildable/measurable independently. Lives at `benchmarks/scenarios/<name>/`. First and only built scenario: `startup-minimal`. The harness (`run.sh <scenario>`) is scenario-agnostic; scenarios are the extension point. |
| **Contender** | One runtime-under-test as *conceptually* named (e.g. "Apache Camel standalone", "rust-camel"). A contender may expand into multiple **artifacts** (one per pairing). The report's rows are contenders-within-a-pair. |
| **Artifact** | One concrete built, launchable deliverable: a specific jar, a `quarkus-app/` directory, or a Rust ELF binary. v1 has **6 artifacts** (3 contenders × 2 pairings, minus the JVM-only splits). "6 artifacts in 2 fair pairings" is the canonical framing — *not* "4 contenders". |
| **Pair / Pairing** | A fairness partition of artifacts that share the same *question*. Pair A = "embedded runtime, no route-file parsing". Pair B = "route authored in YAML, parsed at runtime". Comparison is only valid *within* a pair. See §3. |
| **Pairing fairness** | The invariant that every artifact in a pair pays the same category of cost, so the comparison isolates one variable (runtime overhead in A; runtime + parse overhead in B). Violated if, e.g., a Pair A JVM classpath silently auto-discovers a Pair B `routes.yaml`. |
| **Axis** | An orthogonal experimental variable held fixed in v1 but candidate for future variation: payload size, concurrency level, message rate. Not yet exercised — v1 fixes all axes at "single message, no payload, no concurrency". |
| **Metric / Dimension** | A measured quantity. v1 measures three: cold-start wall-clock (ms), peak RSS (KiB), artifact size (MiB). Metrics are grouped into families for planning: M1 (cold-start + RSS, done), M2 (warm p99), M3 (sustained throughput), M4 (memory growth under load). |
| **Matrix** | The full cross-product of {tier × scenario × contender × pairing × metric × axis}. v1 fills exactly one cell family; the "matrix" is the mental model for how the suite grows without restructuring. |
| **Cold-start** | Wall-clock from *immediately before the process is launched* to the moment the "ready" marker is observed. Explicitly includes JVM/process bootstrap (fork, classloading, JIT init). "First call after process fork", page-cache warm. NOT "first-ever invocation on a freshly booted machine". |
| **Warm-state** | (Future / not measured in v1.) The runtime after JIT compilation and class-init lazy paths have been exercised — the state in which p99 request latency is meaningful. |
| **Steady-state** | (Future.) The regime under sustained constant load where GC pauses, allocator behavior, and throughput plateau become observable. |
| **Sustained throughput** | (Future, M3.) Messages/sec a route processes under continuous load, not a one-shot startup number. |
| **Marker** | The exact stdout string `BENCH_ROUTE_READY`, printed exactly once by every artifact when its route is live. The harness's clock stops on the marker. Every fixture prints ONLY the marker — no self-timing (see §2). |
| **Fixture** | The per-artifact source + build config that produces one artifact and emits the marker. E.g. `camel-standalone-dsl/App.java`, `rust-camel-lib/src/main.rs`, `rust-camel-cli/Camel.toml` + `routes/startup-minimal.yaml`. |
| **Harness** | The scenario-agnostic orchestrator `benchmarks/harness/run.sh`: resolves argv per contender, launches under GNU `time`, polls for the marker, kills, aggregates median + p95. Owns the single clock and all statistical logic. |
| **EIP** | Enterprise Integration Pattern — the route-building vocabulary (filter, choice, split, transform, aggregate…). In v1 only the trivial `log` step is exercised; Tier 2 would introduce real EIPs as the workload. |
| **Route** | The `from -> …steps… -> to` pipeline under test. v1 route: `timer:bench?repeatCount=1&delay=0 -> log`. T2 route: `timer -> set_body -> set_header -> filter -> choice/when/otherwise -> set_body -> log` (5 distinct EIPs). |
| **Component** | A URI-scheme adapter (`timer`, `log`, `exec`, …). Fixtures register only the components their route needs; unused component registration is "dead weight" that inflates RSS (see §4). |
| **Bean** | (Camel term.) A registered object a route can invoke. Not exercised in v1 or v2; relevant to future scenarios that measure bean-binding overhead. |
| **Transform** | An EIP step that mutates the message body. Exercised in v2 T2 (`set_body` EIP is the body-mutating step; `set_header` mutates headers). |
| **Substrate VM** | The GraalVM native-image runtime (also called "the native image runtime"). A closed-world AOT-compiled executable, no HotSpot JVM. `VmHWM` (peak RSS) is what `time -v` reads; Substrate VM's memory model differs from HotSpot (no JIT compiler footprint, no class metadata lazy-loading). Cold-start in Substrate VM shifts work from runtime to **build time** (native-image compilation), so a small post-build cold-start delta reflects deployment economics, not pure runtime cost — see "Native initialization caveat" in the v2 report. |
| **Native initialization** | The build-time cost of generating a native-image binary. For the v2 Quarkus native artifacts, ~64-77 seconds per binary (per `benchmarks/spike-results.md` Spike 1B); the resulting cold-start (~18 ms) is dramatically lower than JVM mode (~540 ms), but **the cost of changing the route is re-running the native build**, not restarting a process. This is the trade-off that the v2 "Native initialization caveat" section of the report makes explicit. |
| **Fingerprint caching** | The v2 harness's mechanism for skipping a native-image rebuild when the inputs that determine the binary are unchanged. A SHA-256 hash of (shared JVM sibling's `src/main/`, native subproject's `build.gradle.kts`, parent `settings.gradle.kts`, JVM sibling's `build.gradle.kts`, native subproject's `application.properties`, `gradle/` wrapper contents, `$JAVA_HOME` path, `native-image --version` output) is stored at `<native-subproject>/.bench-fingerprint`. The build is skipped if the fingerprint matches AND the runner binary exists. A fingerprint mismatch invalidates the cache (forces a rebuild). Fingerprint caching is a **build-time** optimization; the measured numbers are post-build cold-start, so fingerprint caching does not affect the reported medians/p95s. |
| **Pair A predicate deviation** | The T2 Pair A cross-runtime predicate difference: Apache Camel uses Simple language (`simple("${body} == 'ping'")`), rust-camel-lib uses closure predicates (`|ex| ex.body().as_str() == Some("ping")`). The two mechanisms are not language-subsystem-equivalent; T2 Pair A measures "overall EIP pipeline overhead at each framework's idiomatic surface", not "language-subsystem overhead". Pair B is language-subsystem-equivalent (both runtimes parse `${body}` from YAML). Documented explicitly in the v2 report so Pair A claims are scoped accordingly. |
| **ICP (Ideal Customer Profile)** | The named deployment shape for which a benchmark result is a *qualifying* argument rather than bragging. A startup benchmark without a named ICP is meaningless (in a long-lived ESB, JVM startup is irrelevant). v1 chose two ICPs and rejected two — see §5. |
| **Container-hosted cold-start** | (v3.5) M1 measurement taken inside a long-lived Mandrel-based container. Excludes image pull, container creation, k8s scheduling. All cells share identical container tax → relative comparison preserved. |
| **Mandrel-único** | (v3.5) Single Dockerfile based on the Quarkus Mandrel builder image, with Rust toolchain + Maven + bench utilities layered on. One container does the entire build + measure pipeline. Replaces the prior 4-container approach (runner + rust-builder + maven + gradle-native). |

### Self-grill record — §1 Glossary

**Questions generated:**
1. [glossary] Does "artifact" conflict with any existing rust-camel term, and is "6 artifacts" vs "4 contenders" a real distinction or noise?
2. [sharpen] "Tier" and "metric" both sound like "levels" — are they two concepts hiding, or the same axis?
3. [scenario] If a reader sees Pair A has 3 rows but the suite claims "6 artifacts / 3 contenders", can they reconstruct the count?
4. [cross-ref] Does the marker string and "prints exactly once" claim match the actual fixture code?
5. [v2] New terminology from v2 (Substrate VM, native initialization, fingerprint caching, Pair A predicate deviation) — does each have a stable, citable definition in the project, or are they ad-hoc?

**Answers (with citations):**
1. [glossary] No conflict — `CONTEXT-MAP.md` has no "artifact"/"contender" term. The distinction is real and load-bearing: the plan's Goal line explicitly says "6 artifacts in 2 fair pairings (originally framed as 4 contenders)" (`plans/2026-07-18-rc-f3g9-startup-benchmark.md` Goal). Documenting both terms prevents the "4-row flat table" error the pairing design exists to avoid.
2. [sharpen] Two concepts. Tier = workload complexity (route topology); Metric = measured quantity. `benchmarks/README.md` future-scenarios block lists throughput/memory-under-load as *scenarios* (tier-like) while the plan separately tracks cold-start/RSS as *metrics*. They cross-product — a T2 EIP scenario could still be measured on M1 cold-start. Kept as distinct glossary rows.
3. [scenario] Reconstruction: v1: 3 contenders (Camel-standalone, Camel-Quarkus, rust-camel). JVM contenders split A/B (2 artifacts each = 4); rust-camel splits into lib (A) + CLI (B) = 2. Total 6. v2: same 3 contenders, each expands to 4 artifacts (Pair A + Pair B × {JVM, native}) = 12 plus 2 (rust-camel has no JVM mode) = 8 total artifacts per scenario × 2 scenarios = 16 cells. Report's 4 rows per pair × 2 pairs = 8 per scenario × 2 scenarios = 16 (`docs/benchmarks/2026-07-18-benchmark-v2.md` Results tables). Consistent.
4. [cross-ref] Confirmed: `rust-camel-lib/src/main.rs:49` emits `.log("BENCH_ROUTE_READY", LogLevel::Info)`; harness asserts `marker_count -ne 1` is a hard failure (`run.sh` measure_once). Marker contract is real. T2 marker is the same `BENCH_ROUTE_READY` token but with `body=pong-bench` suffix on the same line — the harness `grep -cF` counts the exact full string.
5. [v2] Each new term has a stable, citable definition: Substrate VM is the GraalVM native-image runtime (`spike-results.md` Spike 1B); native initialization is the build-time shift (v2 report "Native initialization caveat" section); fingerprint caching is `run.sh` `compute_and_check_fingerprint` (harness line ~380); Pair A predicate deviation is the spec §4.1 document. All four are recorded as glossary rows so v3+ contributors can find them without re-reading the spec/plan.

**Outcome:** confirm (glossary lands as written; "artifact vs contender" distinction is load-bearing, not pedantic). v2 added 4 new terms with stable definitions.

---

## 2. Architecture / Methodology Decisions (with rationale)

Each row is a decision that looks arbitrary from outside but has a specific
reason. Full arbitration history: plan doc "Measurement design correction"
(6 blessing rounds, e_opus + e_gpt).

| Decision | Rationale |
|---|---|
| **Container-hosted cold-start (v3.5)** | M1 measures "application process cold-start inside a pre-provisioned container environment" — explicitly excluding image pull, container creation, k8s scheduling, and CI runner provisioning. All cells pay identical container tax (process startup, ld.so, libc — same container, same namespace, so it cancels in the relative comparison). The Mandrel-único runner image (`benchmarks/runner/Dockerfile`, based on `quay.io/quarkus/ubi-quarkus-mandrel-builder-image:jdk-21`) hosts both the build and the measurement in one long-lived container; host requires only docker. Replaces v1 "Build in Docker, run bare-metal on host": bare-metal required host-installed JDK + cargo + NixOS-specific paths, neither CI-portable nor reproducible. |
| **Single harness-side clock (no in-process timing)** | Two rejected alternatives bracket the truth: (1) timing `docker run` from *outside* over-counts (container overhead); (2) self-instrumenting from inside `main()` under-counts (skips JVM creation, bootstrap classloading, JIT init — all *before* user code, and all part of real cold-start). A single wall-clock captured immediately before `exec` and stopped at the marker captures full bootstrap with no double-clock fragility. Fixtures therefore carry **zero self-timing** — they only print the marker. |
| **GNU `time -v` for peak RSS** | `Maximum resident set size` from `/usr/bin/time -v` is a *peak*, not an instantaneous sample. An instantaneous `/proc` read can land in a GC valley and under-report a JVM. Peak is the honest worst-case footprint. |
| **`/run/current-system/sw/bin/time` on NixOS** | This host is NixOS: there is no `/usr/bin/time`; GNU time lives at `/run/current-system/sw/bin/time`. The bash builtin `time` does NOT support `-v`. The harness probes candidates (`/usr/bin/time`, the NixOS path, `command -v time`) and verifies `-v true` works before selecting — portable across NixOS and FHS hosts. |
| **KILL immediately after the marker** | Peak RSS must exclude graceful-shutdown teardown allocations (Camel's `Main` and rust-camel both allocate during orderly stop). KILL (SIGKILL) the child the instant the marker is seen so the peak reflects "route live", not "route tearing down". TERM→grace→KILL is reserved for the timeout path only. |
| **n=50 measured runs** | At n=20, nearest-rank p95 degenerates to "the 2nd-highest observation" — too noisy to publish. n=50 makes p95 a stable order statistic. (Fix 8.) |
| **3 warmup runs, discarded** | Cold page cache for the JAR/JRE/binary inflates the first run(s), measuring disk I/O not runtime cold-start. Warmups condition the cache. Warmup **failure is FATAL**, not skipped — a failed warmup means the smoke test lied and measured runs would silently drop data. |
| **Randomized (Fisher-Yates) contender order per run** | Avoids systematic thermal/cache bias favoring whichever contender always runs first or last. |
| **Median + nearest-rank p95 (`ceil(0.95*n)`)** | Not mean/stddev: cold-start distributions are right-skewed and mean is dragged by outliers. Nearest-rank is the standard for small-sample percentiles; median is robust to the skew. |
| **Env-var hygiene: unset `JAVA_TOOL_OPTIONS` / `_JAVA_OPTIONS` / `JDK_JAVA_OPTIONS`** | These env vars silently inject GC flags, agents, or startup hooks into any JVM launched from the shell, contaminating cold-start and RSS invisibly. Unsetting them guarantees the measured JVM is the fixture's JVM. |
| **`LC_ALL=C`** | Forces the C locale so `/usr/bin/time -v`'s "Maximum resident set size (kbytes)" label appears in the exact byte form the harness greps for. Some locales reorder or translate the label, breaking the parse silently. Also stabilizes locale-dependent parsers (YAML/JSON/properties). |
| **Workspace-shared `Cargo.lock` (lib ↔ CLI)** | `rust-camel-lib` (Pair A) and `camel-cli` (Pair B) resolve every transitive dep version against the *same* workspace `Cargo.lock`. This guarantees Pair A vs Pair B isolates **parser/CLI overhead only** — not dependency-version drift (tokio, serde, hyper…). Achieved by making the lib fixture a workspace member (§4). |
| **`.cargo/config.toml` with `target-dir = "target"`** | Pins the lib fixture's release binary to `…/rust-camel-lib/target/release/startup-minimal`, the fixed path the harness wiring depends on. **Gotcha:** cargo precedence is `--target-dir` flag > `CARGO_TARGET_DIR` env > `build.target-dir` config. On this NixOS host `CARGO_TARGET_DIR` may point at a shared CI cache (`/home/shared/...`), which *silently overrides* the config. Builds MUST use `env -u CARGO_TARGET_DIR` (or an explicit `--target-dir`). The harness resolves the real binary path via `cargo metadata` run with `CARGO_TARGET_DIR` unset. |
| **ADR-0033 fail-closed `Camel.toml` stub** | rust-camel's CLI enforces ADR-0033 fail-closed startup validation: `ExecBundle` refuses to start unless at least one `exec` profile is configured, *even when no route uses `exec:`*. The Pair B route uses only `timer`+`log`, so the fixture ships a stub profile (`name = "bench-stub"`, `executable = "true"`) purely to pass validation. NixOS note: bare `true` resolves via PATH; `/bin/true` is NOT used (NixOS has no FHS, `/bin/true` does not exist). Loading the stub costs ~µs, immeasurable at ms resolution. |
| **v3.5: Camel Quarkus JVM variants dropped from measurement** | `camel-quarkus-dsl` and `camel-quarkus-yaml` (JVM mode) duplicate the "Camel bootstrap on HotSpot" signal already covered by `camel-standalone-dsl` / `camel-standalone-yaml` — the production Camel classic baseline. v1/v2 reports retain the JVM numbers as native-vs-JVM mode-delta reference (see COVERAGE.md "Removed" section); v3+ no longer re-measures them. Native variants (`-native` suffix) remain because they represent Apache's marketed answer to startup cost — the comparison `rust-camel-lib vs camel-quarkus-dsl-native vs camel-standalone-dsl` is the ICP-qualifying triple. Matrix reduction: 32 → 26 cells. |
| **v3.5: Bridge scenarios stay at 4 artifacts (asymmetric)** | T4a/T4b measure the bridge subprocess tax that rust-camel pays (no native Rust XSLT/XSD engines; must delegate to Java subprocess via gRPC mTLS). The tax is **authoring-format-invariant**: the gRPC subprocess + byte-pinned XML payload are identical regardless of whether the route was authored in Java DSL or YAML. T1/T2 already isolate the YAML-parse overhead (~5ms). Adding `camel-standalone-yaml` + `camel-quarkus-yaml-native` for T4a/T4b would re-measure a known-invariant quantity — recorded as `won't-measure` in COVERAGE.md per e_opus round-5 verdict. Bridge matrix: 4 cells (camel-standalone-dsl + camel-quarkus-dsl-native + rust-camel-lib + rust-camel-cli). |

### Self-grill record — §2 Decisions

**Questions generated:**
1. [glossary] Does "single clock" conflict with any timing vocabulary elsewhere, and is "bare-metal" being used consistently with the report?
2. [sharpen] "Shared target dir" (task brief §4) vs "shared `Cargo.lock`" — is the sharing about the *target directory* or the *lockfile*?
3. [scenario] If a host has `CARGO_TARGET_DIR` set, does the documented build command still land the binary where the harness looks?
4. [cross-ref] Does the ADR-0033 stub description match the actual `Camel.toml`, and is `/bin/true` really avoided?

**Answers (with citations):**
1. [glossary] "Bare-metal" is consistent: report Methodology says "All 6 artifacts run bare-metal on the host (build happens in Docker, execution does not)" (`docs/benchmarks/2026-07-18-startup-minimal-benchmark.md`). "Single clock" = one `date +%s%N` pair in `measure_once`, no in-process counterpart. No conflict with rust-camel runtime vocab.
2. [sharpen] **Two distinct mechanisms, and the task brief conflates them.** (a) `Cargo.lock` is shared because the lib fixture is a *workspace member* → dep-version parity (`Cargo.toml:19` "Sharing the workspace `Cargo.lock`…isolates only parser/CLI overhead — not dep drift"). (b) `.cargo/config.toml` sets a **fixture-LOCAL** `target-dir = "target"` — the opposite of "shared" — so the binary lands at a fixed per-fixture path (`rust-camel-lib/.cargo/config.toml:12-13`). Sharpened into two separate table rows; flagged the conflation as a terminology gap (see final message).
3. [scenario] Constructed: host with `CARGO_TARGET_DIR=/home/shared/rust-camel-target`. Plain `cargo build --release` → binary lands in the shared dir, harness path resolution *misses it*. Documented command is `env -u CARGO_TARGET_DIR cargo build --release`, which neutralizes the override; harness independently calls `cargo metadata` with the var unset to recompute the path. Both sides agree. Correct.
4. [cross-ref] Confirmed against `rust-camel-cli/Camel.toml:29-31`: `[[default.components.exec.profiles]]` / `name = "bench-stub"` / `executable = "true"`; comment lines 21-22 explicitly warn "do NOT hardcode `/bin/true` (NixOS has no FHS)". Description matches code exactly.

**Outcome:** refine (split the conflated "shared target dir / shared Cargo.lock" into two rows; the sharing is of the *lockfile*, the target-dir is *fixture-local*).

---

## 3. Pairing Model (the key fairness decision)

The single most important design decision. A naïve benchmark would put
Camel's hardcoded Java-DSL route in one row against rust-camel's YAML-parsing
CLI in another — but that mixes **two different questions**: "which runtime is
faster?" and "what does the YAML authoring layer cost?". The pairing model
splits them.

| | **Pair A — embedded runtime, no file parsing** | **Pair B — YAML route, parsed at runtime** |
|---|---|---|
| **Question answered** | Pure runtime bootstrap overhead | Runtime + config-parse overhead |
| Camel standalone | `camel-standalone-dsl/` — `App.java`, hardcoded Java-DSL, **no** `yaml-dsl` on classpath | `camel-standalone-yaml/` — `AppYaml.java` loads `routes.yaml` via `camel-yaml-dsl` |
| Camel Quarkus | `camel-quarkus-dsl-native/` — hardcoded `BenchRoute.java`, native image (v3.5: JVM variant dropped — redundant with `camel-standalone-dsl`) | `camel-quarkus-yaml-native/` — YAML route + native image (v3.5: JVM variant dropped) |
| rust-camel | `rust-camel-lib/` — programmatic route via **public `CamelContext` API**, no CLI/YAML | `rust-camel-cli` — `camel run` + `startup-minimal.yaml` |

**Why the split is enforced structurally (not just by convention):** each JVM
fixture ships as **two separate build artifacts** (Maven modules / Gradle
subprojects), because a *shared* jar/classpath would let Camel Main's
auto-discovery silently load Pair B's `routes.yaml` from Pair A's classpath —
invalidating the "no parsing" claim without any visible error. Fairness is a
classpath-isolation property, not a code-comment promise.

**Why rust-camel appears as lib in A but CLI in B — two different entrypoints,
by design:**

- `rust-camel-lib` (Pair A) exercises the **public `CamelContext` lifecycle
  API** directly: construct context → register `TimerComponent` → build route
  programmatically → start → marker. This is the "embedded library" shape,
  the honest analog of Camel's hardcoded Java-DSL. Requiring it forced the
  public lifecycle API to be usable standalone (or the benchmark would STOP).
- `rust-camel-cli` (Pair B) exercises the **CLI bootstrap** (`camel run
  --config Camel.toml --routes …yaml`), which pays for TOML config load,
  ADR-0033 startup validation, and YAML route parsing — the honest analog of
  Camel loading `routes.yaml` via `camel-yaml-dsl`.

The two rust-camel artifacts are **not** the same binary measured twice: the
lib is a 4.8 MiB single-purpose ELF; the CLI is the 86 MiB general-purpose
`camel` binary. This asymmetry is itself a documented limitation (§6), not a
flaw in the pairing — within each pair the comparison is apples-to-apples.

### Self-grill record — §3 Pairing

**Questions generated:**
1. [glossary] Does "Pair" collide with any rust-camel term (Exchange pairing, request/reply)?
2. [sharpen] "Runtime overhead" vs "parse overhead" — are these cleanly separable, or does Pair A still hide some parse cost?
3. [scenario] What input breaks pairing fairness, and does the design actually prevent it?
4. [cross-ref] Do the fixtures actually match the A/B assignment (does rust-camel-lib really avoid YAML)?

**Answers (with citations):**
1. [glossary] No collision. `CONTEXT-MAP.md` has no "Pair" term; the closest is request/reply consumer adapters, a different domain. Benchmark "Pair A/B" is unambiguous in context.
2. [sharpen] Cleanly separable *by construction*: Pair A pays zero route-file parse (hardcoded/programmatic route); Pair B pays runtime-parse of an identical route. The *difference* B−A is the parse cost. Pair A still includes component-registration cost, which is why dead-weight component removal matters (§4) — but that is "runtime overhead" by definition, correctly in A. Sharpened: A = runtime bootstrap incl. component registration; B = A + file parse.
3. [scenario] Breaking input: put `routes.yaml` on Pair A's classpath → Camel Main auto-discovers and parses it, silently converting A into B with no error. Prevented by shipping A and B as *separate* Maven modules / Gradle subprojects with disjoint classpaths (`benchmarks/README.md` Contenders; e_gpt Fix 4). Design holds.
4. [cross-ref] Confirmed: `rust-camel-lib/src/main.rs` imports only `TimerComponent` and builds the route programmatically with `RouteBuilder` + `.log(...)` — no YAML, no serde_yaml path (`main.rs:22,40,49`). Pair A assignment is real, not aspirational.

**Outcome:** confirm (pairing model stands; sharpened the "runtime vs parse" boundary to explicitly include component registration in A).

---

## 4. Fairness Fixes Applied

Concrete corrections made during v1 so the pairing held. Each is a place where
the *default* setup would have quietly biased a result.

| Fix | What / why |
|---|---|
| **`rust-camel-lib` added to root workspace `[members]`** | Root `Cargo.toml:17` lists the fixture under `members` but **excludes it from `default-members`** (`Cargo.toml:19`), so `cargo build` at root ignores it, yet it still resolves against the shared workspace `Cargo.lock` → dep-version parity with `camel-cli` (Pair B). Without membership, the fixture would resolve its own dep versions and Pair A vs Pair B would measure dep drift, not parse cost. |
| **`LogComponent` dead-weight removed from the lib fixture** | The `timer -> log` route's `.log()` EIP step resolves directly to `camel_processor::LogProcessor` — it does **not** require registering a `LogComponent`. An earlier fixture draft registered `LogComponent` needlessly, adding dead-weight to startup and RSS. Current `main.rs` imports only `TimerComponent` (`main.rs:22,40`). Every registered-but-unused component inflates the very numbers being measured. |
| **`.cargo/config.toml` with `target-dir = "target"`** | Fixture-LOCAL target dir pins the release binary to a fixed path the harness reads. (Note: this is *not* a shared target dir — see §2 gotcha. The *lockfile* is shared; the *target dir* is local.) |
| **Camel Quarkus YAML route at `src/main/resources/camel/routes.yaml`** | Camel Quarkus auto-discovers routes at `classpath:camel/*` via Camel Core's `DefaultConfigurationProperties` — **NOT** `routes.yaml` at classpath root. Putting the file at the root would mean Pair B's Quarkus artifact silently loads *no* route, never emits the marker, and either fails or (worse) measures a runtime with nothing to parse. |
| **Quarkus fast-jar layout understanding (`app/*.jar`, not `quarkus/root.jar`)** | Quarkus 3.20 fast-jar layout puts app resources in `app/*.jar`; `quarkus/root.jar` does not exist in this layout, and `quarkus-run.jar` is a thin launcher. The runtime-parsing verification step must scan `app/*.jar` (fallback `lib/*.jar`) to confirm `routes.yaml` is present as a loadable resource — otherwise the "YAML parsed at runtime" claim (the whole point of Pair B) is unverified. |

### Self-grill record — §4 Fairness fixes

**Questions generated:**
1. [glossary] Is "dead weight" a defined term, or informal? Does it need a glossary entry?
2. [sharpen] The task brief says `.cargo/config.toml` gives a "shared target dir" — is that accurate?
3. [scenario] If the Quarkus YAML were at the classpath root instead of `camel/`, what would the benchmark measure?
4. [cross-ref] Is `LogComponent` actually removed in the current fixture, or does the import still exist?

**Answers (with citations):**
1. [glossary] Informal but useful — folded into the "Component" glossary row ("unused component registration is dead weight that inflates RSS") rather than a standalone term. Not load-bearing enough for its own entry.
2. [sharpen] **Inaccurate as phrased.** `.cargo/config.toml:12-13` sets `target-dir = "target"` which is *fixture-local*, the opposite of shared. The genuinely *shared* thing is the workspace `Cargo.lock`. Documented the correction in §2 and §4; flagged in final message.
3. [scenario] Constructed: `routes.yaml` at classpath root, Quarkus scans `classpath:camel/*` → finds nothing → route never registers → no marker → harness `marker_count != 1` → hard failure (or, if some other route existed, it would measure the wrong route). The `camel/` subdir placement is load-bearing (`plans/…` Task 3 R3-Fix corrections).
4. [cross-ref] Confirmed removed: current `main.rs` imports only `use camel_component_timer::TimerComponent;` and registers only `TimerComponent` (`main.rs:22,40`); the `.log()` step is noted to resolve to `camel_processor::LogProcessor` (`main.rs:37`). No `LogComponent` registration in current state. (A stale mid-session snapshot showed the import; the committed fixture does not — verified against disk.)

**Outcome:** refine (correct the "shared target dir" mischaracterization; fold "dead weight" into the Component glossary row instead of a new term).

---

## 5. ICP Framings

A startup benchmark is only a *qualifying* argument if the Ideal Customer
Profile is named (parity analysis §3.12: "In a long-lived ESB, 2s of JVM
startup is irrelevant — the JVM runs forever"). v1 chose two ICPs and rejected
two, driven strictly by the measured numbers.

### Chosen

| ICP | Data that supports it |
|---|---|
| **Dev inner-loop / CI** (Pair A: ~8 ms cold-start, ~6 MiB RSS, 4.8 MiB binary) | An edit-run-inspect loop for Camel-style integration code costs ~300–600 ms/iteration just to fork a JVM and parse the route; the lib path costs ~8 ms. That is the difference between an interactive watch-reload loop and a `mvn test` coffee-break. Binary small enough to ship as a test-harness sidecar without bloating images. |
| **K8s deployment density / dev-sidecar** (Pair B: ~13 ms cold-start, ~32 MiB RSS, 86 MiB CLI binary) | 32 MiB RSS × 50 sidecars/node = 1.6 GiB, well under typical sidecar budgets. The YAML parse path adds only ~5 ms over the lib path, so "ship a new route with no code change" is essentially free at this scale. Caveat: the 86 MiB CLI binary is too large to embed in *every* container — one sidecar per node serving many pods is the viable shape. |

### Rejected

| ICP | Why the data does NOT support it |
|---|---|
| **Edge computing** | Edge devices generally do not have 32 MiB to spare for a mostly-idle sidecar, and the 86 MiB CLI binary is too fat for edge. The genuine edge-adjacent data point is Pair A (4.8 MiB / 8 ms embedded lib with a single hardcoded route) — but that is a "compute-constrained device, one hardcoded route" use case, not a general "edge integration gateway". Framing it as edge would misrepresent the deployment footprint. |
| **Scale-to-zero / function cold-start** | The advantage is real but not "millisecond-frugal": Pair B is 13 **ms**, not 13 µs. Scale-to-zero economics also care about warm p99 (M2), not just first-call cold-start — a metric v1 does not measure. Marketing an 8 ms cold-start as a scale-to-zero win is a solution seeking a problem the data hasn't sized. |

**Canonical single-ICP statement (from the report):** rust-camel's primary
near-term ICP is **dev inner-loop / CI** for Camel-style integration code,
with a secondary **K8s-sidecar** shape for teams needing dynamic route loading
without a JVM in the request path.

### Self-grill record — §5 ICP

**Questions generated:**
1. [glossary] Is "ICP" used consistently between the parity analysis and the report?
2. [sharpen] "K8s sidecar" — is it one ICP or two (density vs dynamic-route-loading)?
3. [scenario] Does the rejection of scale-to-zero survive if a future warm-p99 metric (M2) shows a strong number?
4. [cross-ref] Do the ICP numbers (8 ms / 6 MiB, 13 ms / 32 MiB) match the published tables?

**Answers (with citations):**
1. [glossary] Consistent. Parity analysis §3.12 defines the conditioning ("benchmark is only a weapon if the ICP is named"); the report §ICP delivers named ICPs with the same vocabulary. No drift.
2. [sharpen] One ICP with two facets: "deployment density" (RSS budget) and "dynamic route loading without a JVM in the request path" (the YAML-parse-is-free property). Kept unified in the table with both facets noted — splitting would over-fragment a single deployment shape.
3. [scenario] Constructed: if M2 later shows rust-camel warm p99 crushing JVM p99, scale-to-zero could *re-enter* as a candidate — but that is a **future-metric** question, framed as open in §7 (M2). v1's rejection is scoped to "on the M1 cold-start data alone", which is honest. The rejection is not permanent; it is data-scoped.
4. [cross-ref] Confirmed: Pair A row "rust-camel (embedded library) 8.0 / 9 ms, 5 956 KiB RSS, 4.8 MiB" and Pair B "rust-camel (CLI+YAML) 13.0 / 14 ms, 31 960 KiB, 86 MiB" (`docs/benchmarks/2026-07-18-startup-minimal-benchmark.md` Results). 6 MiB ≈ 5 956 KiB, 32 MiB ≈ 31 960 KiB. Numbers match.

**Outcome:** confirm (ICP framings stand; noted explicitly that the scale-to-zero rejection is *data-scoped to M1*, re-openable if M2 lands — cross-linked to §7).

---

## 6. Honest Limitations

Stated plainly so no reader over-reads the numbers. These are the "not
directly comparable" caveats, promoted to first-class context.

| Limitation | Why it matters |
|---|---|
| **Trivial scenario (T1: `timer -> log`)** — **PARTIALLY RESOLVED in v2** | v1's T1 exercises almost no EIP surface. v2 adds T2 (`t2-realistic-eip`) with 5 distinct EIPs (`set_body`, `set_header`, `filter`, `choice`, `log`). Neither scenario covers bean / process registration, external I/O (kafka, http, file), multi-route error handling, or payload-size / concurrency axes. The "trivial" tag now applies to T1 only; T2 is the "minimal real route" scenario and is the more informative data point for generalization. |
| **JVM mode only for Quarkus (no native-image)** — **RESOLVED in v2** | v1's Quarkus artifacts were JVM-mode only, which under-sold Quarkus on the axis it optimizes. v2 adds 2 native-image artifacts (`camel-quarkus-dsl-native`, `camel-quarkus-yaml-native`) per scenario (4 cells total). The v2 numbers show Quarkus native is ~30× faster on cold-start and ~3.3× smaller on RSS than Quarkus JVM — a real Quarkus win, now visible in the report. **The v1 unfairness is closed.** A new related limitation takes its place: **native-image is build-time-expensive** (64-77 s per binary on this host per `spike-results.md` Spike 1B), so the cold-start-vs-iteration-cost trade-off is named explicitly in the v2 report's "Native initialization caveat" section. |
| **rust-camel-cli built with default features (86 MiB)** | The Pair B binary is the full general-purpose `camel` binary, statically linked against the entire workspace (Tower, hyper, tokio, serde_yaml, reqwest, exec component, +12 crates) even though the route uses only `timer`+`log` (T1) or `timer`+`set_body`+`set_header`+`filter`+`choice`+`log` (T2). Not representative of a minimal CLI. A `--no-default-features --features minimal` build would land near the 4.8 MiB lib. |
| **Bare-metal loses container isolation** | Noisy-neighbor effects on the host hit all 8 (v2) artifacts equally, so they **cancel in the relative comparison** — but absolute medians are host-load-dependent and may shift a few ms on a quieter host. The v2 run had a host load average of 3.7 (per `/proc/loadavg` at run end), which is noisier than the v1 run (~1). Rust RSS is 5-10% lower in v2 than v1 — within run-to-run noise. The relative multipliers between contenders (rust-camel vs Quarkus native vs Quarkus JVM) are robust; the absolute ms are not. |
| **n=50 has statistical power for large effect sizes only** | The v2 effect sizes range from 1.4× (rust-camel-cli vs Quarkus native, Pair B cold-start) to 30× (Quarkus native vs Quarkus JVM, cold-start). The 30× and 2.3× gaps survive any reasonable noise. The 1.4× Pair B gap is borderline — n=50 cannot resolve a 5% difference between two similar contenders but can resolve 30%+ effects. The suite is built to demonstrate order-of-magnitude gaps, not fine distinctions. |
| **rust-camel-cli ADR-0033 stub is a fixture artifact** | The `Camel.toml` exec stub exists only to pass fail-closed validation; it slightly diverges from a "clean" minimal config, though its cost is immeasurable (~µs). A reader should not infer that a real minimal rust-camel deployment needs an exec profile. |
| **T2 is EIP-only (no bean, no process, no multi-exchange, no error handling)** — **NEW in v2** | T2 exercises 5 distinct EIPs but does not register a bean, invoke a `process` step, split an exchange, or handle a routing error. The `set_body` / `set_header` / `filter` / `choice` / `log` pipeline is the simplest "real route" shape. A real integration route that uses a bean or external component is **not directly covered by these numbers** — RSS will grow as components are registered, and cold-start will grow as init paths are exercised. The scale of the growth is not measured here. |
| **Pair A predicate mechanism differs from Apache Camel in T2** — **NEW in v2** | rust-camel-lib Pair A uses closure predicates (`|ex| ex.body().as_str() == Some("ping")`); Apache Camel Pair A uses Simple language (`simple("${body} == 'ping'")`). Not language-subsystem-equivalent. T2 Pair A measures "overall EIP pipeline overhead at each framework's idiomatic surface", not language-subsystem equivalence. Pair B is language-subsystem-equivalent. Documented explicitly in the v2 report. |

### Self-grill record — §6 Limitations

**Questions generated:**
1. [glossary] Is "native-image" defined anywhere in-project, or does it need context for a Rust-native reader?
2. [sharpen] "Statistical power for large effect sizes only" — is that a rigorous claim or hand-waving?
3. [scenario] Could a hostile reader use the 86 MiB CLI number to claim rust-camel is *bloated* vs Camel's 6 MiB jar?
4. [cross-ref] Does the report actually disclose all these, or is this doc adding new caveats?
5. [v2] Two of the v1 limitations are resolved (JVM-only Quarkus, trivial scenario); how are the resolutions documented here vs in the v2 report?

**Answers (with citations):**
1. [glossary] Native-image is a GraalVM/Quarkus term (AOT compilation to a static native binary), not a rust-camel term — added a one-clause gloss inline in the limitation row so a Rust-native reader understands why it changes the comparison. It is the crux of v2, so worth spelling out.
2. [sharpen] Rigorous enough for context: the observed effects are ~40× (multiplicative), where even a 2× measurement error leaves the conclusion intact; n=50 cannot resolve sub-10% differences but is never asked to. Sharpened to "demonstrates order-of-magnitude gaps, not fine distinctions".
3. [scenario] Constructed hostile read: "rust-camel is 86 MiB vs Camel's 6 MiB jar — it's bloated." Rebuttal is in the report: the 86 MiB is the *general-purpose* binary (Camel's 6 MiB jar excludes the ~150 MiB JVM it needs to run; rust-camel needs no runtime). The fair Rust number for a single route is the 4.8 MiB lib. The caveat exists precisely to defuse this. Documented (`report` "Not directly comparable" + "rust-camel-cli binary is 86 MiB").
4. [cross-ref] All six v1 limitations are in the v1 report's "Not directly comparable" / "ICP" sections; this doc *synthesizes and promotes* them to context, not inventing new ones. v2 adds 2 new limitations (T2 EIP-only, Pair A predicate deviation) and marks 2 as resolved/shifted (JVM-only Quarkus, trivial scenario).
5. [v2] Resolutions are documented in two places: (a) the v2 report's "Delta table vs v1" makes the native-image resolution quantitative; (b) this §6 marks the status with `**RESOLVED**` / `**PARTIALLY RESOLVED**` / `**NEW**` tags so a v3 reader can see what v2 changed without re-reading the report. The build-time cost of native is now a sub-row of the "JVM mode only" row, not a separate top-level limitation, because it is the same trade-off (faster cold-start ↔ slower build/iteration).

**Outcome:** confirm (limitations stand; added inline native-image gloss for Rust-native readers; sharpened the n=50 power claim). v2 marked 2 v1 limitations as resolved/partially-resolved and added 2 new ones for T2-specific concerns.

---

## 7. Open Design Space for v2+ (questions, not recommendations)

Framed as "what would need to be true for X to be the right next step?" — this
doc does **not** recommend scope. That is the next phase (brainstorm + spec).

### Native-image scope — **RESOLVED in v2**

- **Q:** ~~What would need to be true for native-image to be added to *only* Tier 1, vs Tier 1 **and** Tier 2?~~ — **Resolved:** v2 added native-image to **both** Tier 1 and Tier 2 (4 native Quarkus cells: 2 scenarios × 2 native artifacts). The fairness goal and the workload-realism goal were both pursued. AOT behavior under real EIP workloads is now measurable; the v2 numbers show native cold-start is unchanged T1→T2 (~18 ms both), confirming the closed-world analysis absorbs the EIP pipeline cost. Open follow-up: **should Quarkus native be added to Tier 3 (external deps)?** — that depends on whether the ICP shifts toward "real I/O in the route" (which native AOT *might* handle differently from JVM — registration-time vs first-call init).

### Scenario tiers

- **Q:** ~~What would need to be true for Tier 2 (real EIPs: filter/choice/split/transform) to be the right next scenario?~~ — **Resolved:** v2 added `t2-realistic-eip` with 5 EIPs (`set_body`, `set_header`, `filter`, `choice`, `log`). Split and transform are not in T2 (split requires multi-exchange handling; transform is a synonym for `set_body` in the Camel vocabulary). Follow-up for v3+: **should T2 be extended to split + aggregate** to cover the "one-to-many" EIP surface? — that requires a multi-message harness shape (currently single-message, repeatCount=1).
- **Q:** For Tier 3 (external deps: kafka/http)? — That the ICP shifts toward "does the runtime hold up with real I/O in the route", which cold-start cannot answer; requires a broker/server in the harness (a materially more complex, less reproducible setup).
- **Q:** For Tier 4 (multi-route + error handling)? — That supervision/error-disposition overhead (ADR-0018/0019) becomes a claimed differentiator worth measuring.

### Metric expansion

- **Q:** What would need to be true for M2 (warm p99, no-GC latency determinism) to be next? — That an ICP requiring *warm* latency (request-serving sidecar, not cold-start) becomes primary. This would also re-open the scale-to-zero ICP (§5), currently rejected *on M1 data alone*.
- **Q:** For M3 (sustained throughput, msgs/sec)? — That "throughput under load" becomes a claimed differentiator; requires a load generator and steady-state detection in the harness.
- **Q:** For M4 (memory growth under load, GC-pause visibility)? — That deterministic memory / no-GC becomes the headline (parity analysis names this as a rust-camel differentiator beyond cold-start).

### Build-profile variation

- **Q:** What would need to be true for a rust-camel-cli slim build (`--no-default-features --features minimal`) to be worth adding? — That the 86 MiB binary is judged to *mislead* readers about the minimal deployable, strongly enough to justify maintaining a second build profile purely for the benchmark. (The report already names this as a "future build-profile task, not a benchmark re-run".)

### Axis variations

- **Q:** What would need to be true for payload-size / concurrency axes to be exercised (v3?)? — That an ICP cares about behavior *under varying load shape*, not fixed single-message cold-start. These multiply the matrix and are explicitly deferred past v2 in the current framing.

### Self-grill record — §7 Open design space

**Questions generated:**
1. [glossary] Do "Tier 2/3/4" and "M2/M3/M4" labels risk being read as commitments rather than options?
2. [sharpen] "What would need to be true for X" — is every bullet genuinely a *condition*, or did any slip into a recommendation?
3. [scenario] Does any open question here contradict a limitation in §6 (e.g. proposing something §6 says is unfair)?
4. [cross-ref] Are the tier/metric labels sourced from the plan/README, or invented here?
5. [v2] With T2 + native-image resolved, what is the *next* open question worth naming? Is it Tier 3 (external deps), Tier 4 (multi-route), or M2 (warm p99)?

**Answers (with citations):**
1. [glossary] Risk exists; mitigated by the section preamble ("questions, not recommendations") and by phrasing every bullet as a condition. The T/M labels are *organizing handles* from the brief and `benchmarks/README.md` future-scenarios block, not a roadmap. Kept, with the preamble as guard.
2. [sharpen] Audited each bullet: all are conditionals ("would be … if", "that … becomes primary"). The slim-build bullet quotes the report's own "future task" language rather than asserting a recommendation. No recommendation leaked. (Re-read confirmed.)
3. [scenario] Cross-checked §6 ↔ §7: §6 says JVM-only-Quarkus is unfair → §7 native-image Q addresses *fixing* that (consistent, not contradictory, **now resolved in v2**). §5 rejects scale-to-zero on M1 → §7 M2 Q explicitly notes M2 would *re-open* it (consistent, cross-linked). No contradiction.
4. [cross-ref] Tier/metric families trace to the brief's §7 and `benchmarks/README.md`'s "future scenarios" block (throughput-sustained, memory-under-load) plus the report's "Throughput and memory-under-load are future scenarios". Not invented — synthesized from existing sources.
5. [v2] The next-open question is **M2 (warm p99)** because (a) the v2 report's "Findings narrative" explicitly notes the Quarkus native gap to rust-camel is "small enough that a future workload or future Quarkus version could close it" — warm p99 would re-open the scale-to-zero ICP (currently rejected on M1 alone) if rust-camel's no-GC and no-JIT advantage compounds; (b) the rust-camel Rust advantage under sustained load is a different *kind* of advantage than cold-start, and the suite has not measured it. Tier 3/4 are also open but are about route shape, not about ICP-relevant data.

**Outcome:** confirm (open questions stand; v2 marked T2 + native-image as resolved, named M2 as the next-open question worth pursuing).

---

## Cross-references

- Harness design + methodology summary: `benchmarks/README.md`
- Published v1 report with real numbers: `docs/benchmarks/2026-07-18-startup-minimal-benchmark.md`
- Published v2 report (native-image + T2): `docs/benchmarks/2026-07-18-benchmark-v2.md`
- Full plan + 6-round methodology arbitration history (v1): `docs/superpowers/plans/2026-07-18-rc-f3g9-startup-benchmark.md`
- v2 plan: `docs/superpowers/plans/2026-07-18-rc-p9ki-benchmark-v2.md`
- v2 spec: `docs/superpowers/specs/2026-07-18-rc-p9ki-benchmark-v2-design.md`
- Spike results (NixOS native build workarounds): `benchmarks/spike-results.md`
- ICP conditioning (why a named ICP is mandatory): `docs/superpowers/analysis/analysis-apache-camel-parity-2026-06-27.md` §3.12
- ADR-0033 (fail-closed startup validation, the reason for the `Camel.toml` exec stub): `docs/adr/0033-security-defaults-fail-closed-startup-validation.md`
- v1 bd: `rc-f3g9` (closed)
- v2 bd: `rc-p9ki`
