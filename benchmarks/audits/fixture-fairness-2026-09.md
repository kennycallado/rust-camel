# Fixture-Fairness Audit — Comparative Benchmark Suite (2026-09)

- **Date:** 2026-09-16
- **Auditor:** fleet mission 98, bd rc-h42s6
- **Worktree:** `/home/shared/rust-camel-worktrees/benchintegrity` (branch `feature/benchintegrity`)
- **HEAD at audit:** `70857e15db90dfbc58f84cbb1b06d0f598a26c4f` (`70857e15 docs: restructure guides to parity with runtime`)
- **Record provenance:** era-2 record `20260915T093128Z` (published by `63d37493 chore(bench): publish era-2 record 20260915T093128Z`)
- **Scope:** per-cell fixture work inventory for every active scenario × contender; cross-contender asymmetry flags WITHIN each scenario. Read-only: no fixture, harness, `benchmarks/harness/out/**`, or `benchmarks/records/**` file was modified. This document is the only file created.

## Method

- Every roster cell's fixture code was read at HEAD with line numbers. The rule is **code over comments**: where a docstring describes intent ("canonical minimal", "no per-request work") and the code does more (or less), the code wins and the mismatch is recorded in [Contradictions](#contradictions-found).
- Tick fixtures (t2-json, t2-realistic-eip, split-aggregate, both bridges) were traced end-to-end: route steps, per-tick counters, per-tick file appends, and the `BENCH_LATENCY` write mechanism (fixture-direct vs runtime-module vs wrapper).
- Measurement-path files were read for context but not audited line-by-line as fixtures: `benchmarks/harness/run.sh` (cell argv composition, latency-file reader), `crates/camel-cli/src/commands/bench_instrument.rs` (module latency modes), wrapper scripts, `benchmarks/scenarios/*/smoke/run.sh` (http-server only, for the smoke-contract question).
- Comparison validity rules used throughout: flags are only raised WITHIN a scenario and WITHIN a pair (Pair A: `camel-standalone-dsl`, `camel-quarkus-dsl-native`, `rust-camel-lib`; Pair B: `camel-standalone-yaml`, `camel-quarkus-yaml-native`, `rust-camel-cli`). `node-fastify`/`node-native` are their own runtime family — compared fastify-vs-native within the family only. `axum-bare` is a reference cell (rc-u034): deviations are noted but never counted as contender flags. Pair A-vs-B parse/authoring differences are the experimental design (YAML-parse and bridge-tax isolation) and are NOT flagged as unfair.
- Flag taxonomy: `[LOG-ASYM]` stdout log lines per request/tick; `[COUNTER-ASYM]` counter/atomic work; `[BODY-ASYM]` body work (drain/parse/mutate/transform); `[HEADER-ASYM]` header work; `[WRAPPER-ASYM]` wrapper- or module-induced work / measurement-path differences; `[PARSE-ASYM]` parse differences (only within a pair — none found); `[PROTOCOL]` unit differences (none found — protocol cells are never cross-compared here).
- Timer-URI and clock-quality divergences that affect neither per-tick work nor window fairness are recorded as untagged notes.

## Roster verification

Verified against `benchmarks/scenarios/COVERAGE.md`: 5 full scenarios × 8 contenders = 40, 2 bridge scenarios × 6 = 12, plus `axum-bare` (http-server-only reference, "the 53rd roster identity" per COVERAGE.md "Consolidated builds") = **53 cells. All 53 fixture sets are present; nothing is MISSING.**

- COVERAGE.md's artifact-set table lists 4 Camel-side artifacts per full scenario; the node family (`bench-node`) registers both members in all 7 scenarios (COVERAGE.md "Node contender axis"; harness `FAMILY_COMPLETENESS`, `run.sh:1492-1494`). Bridge scenarios carry 6 contenders, not the 4 in COVERAGE.md's line-50 note — that note predates the node axis and refers to the Camel-side artifact set only (the YAML Camel variants are still correctly absent from bridges).
- Quarkus native variants build from the shared JVM sibling's sources via Gradle `sourceSets.srcDir` (per-scenario `camel-quarkus-dsl-native/build.gradle.kts` — e.g. xsd-validation-bridge :11-14, t2-json :22-25, t2-realistic-eip :20-23; harness `run.sh:1399-1401`), EXCEPT http-server, where `camel-quarkus-dsl-native` has its own `NativeBenchRoute.java` (platform-http instead of jetty).
- **multi-step:** NOT an active scenario. COVERAGE.md contains zero mentions of it (the matrix's T7 "Composite multi-hop" is `open-if`, v5+). `benchmarks/scenarios/multi-step/` holds only a partial fixture set (rust-camel-cli `Camel.toml`, `routes/multi-step.yaml`, wrapper, artifact JSONs c16/c300/c1000) with no harness wiring. Excluded from this audit.

## Scenario 1 — startup-minimal (T1, cold-only, protocol B; M1 only)

No per-request or per-tick axis exists in this scenario (single timer fire, `repeatCount=1`). Every marker is the literal line `BENCH_ROUTE_READY` with no suffix; the harness greps `-F`, so the logger-format differences (SLF4J vs tracing vs console) are cosmetic. The per-process differences below ARE the measured quantity (bootstrap cost), not unfairness.

File key: `App.java` = `scenarios/startup-minimal/camel-standalone/camel-standalone-dsl/src/main/java/com/rustcamel/bench/App.java`; `AppYaml.java` = same tree under `camel-standalone-yaml`; `routes.yaml` = `camel-standalone-yaml/src/main/resources/routes.yaml`; `BenchRoute.java` = `camel-quarkus/camel-quarkus-dsl/src/main/java/com/rustcamel/bench/BenchRoute.java`; `qy-routes.yaml` = `camel-quarkus/camel-quarkus-yaml/src/main/resources/camel/routes.yaml`; `startup.rs` = `contenders/rust-camel-lib/src/scenarios/startup-minimal.rs`; `cli.yaml` = `scenarios/startup-minimal/rust-camel-cli/routes/startup-minimal.yaml`; node = `contenders/node/node-{native,fastify}/startup-minimal.mjs`.

| Contender | Pair | Protocol | Work per process startup | Marker mechanism | Flags |
|---|---|---|---|---|---|
| camel-standalone-dsl | A | cold-only | JVM boot + Camel Main; route `timer:bench?repeatCount=1&delay=0` → log step (App.java:27-28) | route log step (SLF4J-formatted), timer-driven | — |
| camel-quarkus-dsl-native | A | cold-only | Quarkus native boot; identical route from shared source (BenchRoute.java:20-21) | route log step | — |
| rust-camel-lib | A | cold-only | tokio runtime + context build; same route programmatic (startup.rs:58-65) | route log step (tracing-formatted) | — |
| camel-standalone-yaml | B | cold-only | JVM boot + YAML route-file parse (AppYaml.java:24-26; routes.yaml:1-9); same route | route log step | — (YAML parse is the Pair B design) |
| camel-quarkus-yaml-native | B | cold-only | Quarkus native boot + YAML parse (qy-routes.yaml:1-9); same route | route log step | — |
| rust-camel-cli | B | cold-only | CLI boot + YAML parse (cli.yaml:18-22); same route; harness runs the bare binary, no wrapper (`run.sh:1795-1798`) | route log step through child stdout | — |
| node-native | node | cold-only | module load only; single `console.log` (node-native/startup-minimal.mjs:26) | direct stdout line; **process exits 0 immediately** (lines 20-24) | [WRAPPER-ASYM] (see below) |
| node-fastify | node | cold-only | fastify() + route registration + `await app.ready()` (avvio boot, no bind), then marker (node-fastify/startup-minimal.mjs:20-33) | direct stdout line; exits 0 | [WRAPPER-ASYM] (exit-0 shape, see below; boot-before-marker is the cell's purpose) |

**Flags:**
- [WRAPPER-ASYM] (measurement-path, node family): node cells exit after the marker while every framework fixture idles until externally killed (node-native/startup-minimal.mjs:20-24 vs App.java:31 `main.run()` blocking). Any post-marker harness sampling (RSS teardown, kill semantics) sees a live process for Camel/rust cells and a dead one for node. Both node cells are unmeasured (`open-if`), so this is latent, not active.

## Scenario 2 — t2-json (T2j, protocol B; 10ms tick × 10000)

Canonical per-tick pipeline: set body (canonical JSON, 32768B default) → stamp start → unmarshal JSON → filter (`id == "bench"`) → insert `"bench": true` → marshal JSON → assert (length `size+13` + semantics) → marker once (`BENCH_ROUTE_READY bytes=<len>`, first tick) → `BENCH_LATENCY <id> <ns>` append. Marker formats are uniform (`bytes=` suffix) across all 8 cells.

File key: `App.java` = `scenarios/t2-json/camel-standalone/camel-standalone-dsl/.../App.java`; `AppYaml.java` = `camel-standalone-yaml/.../AppYaml.java`; `s-routes.yaml` = `camel-standalone-yaml/src/main/resources/routes.yaml`; `BenchRoute.java` = `camel-quarkus/camel-quarkus-dsl/.../BenchRoute.java`; `BenchBeans.java` = `camel-quarkus/camel-quarkus-yaml/.../BenchBeans.java`; `qy-routes.yaml` = `camel-quarkus/camel-quarkus-yaml/src/main/resources/camel/routes.yaml`; `t2json.rs` = `contenders/rust-camel-lib/src/scenarios/t2-json.rs`; `cli.yaml` = `scenarios/t2-json/rust-camel-cli/routes/t2-json.yaml`; node = `contenders/node/node-{native,fastify}/t2-json.mjs`.

| Contender | Pair | Protocol | Work per tick | Work per process startup | Flags |
|---|---|---|---|---|---|
| camel-standalone-dsl | A | B: 10ms tick | setBody(constant); stamp :104-105, Jackson unmarshal :106, jsonpath filter :107, member insert :108, marshal :110; full assert incl. second JSON parse `readTree` :158-190 (parse :169, header set `benchOutLen` :188); marker CAS :115-120; counter incr + direct file APPEND :121-134 | payload build + `BENCH_INPUT_SHA256=<digest>` :76-82; latency file truncate :86-88; timer `delay=0` :97 | — |
| camel-quarkus-dsl-native | A | B: 10ms tick | identical pipeline, BenchRoute.java:79-116 (assert :137-168 incl. readTree :148, header :167, marker :97-102, counter+append :103-116) | same shape (:57-75); timer `delay=0` :79 | — |
| rust-camel-lib | A | B: 10ms tick | set_body(prebuilt) :206, stamp :211-214 (after set_body), unmarshal json :215, closure filter :219-224, map insert :107-117/:225, marshal :227, full serde assert :123-151/:228-242 (marker :237-239), counter + file APPEND :243-258 (per-tick reopen :253); **no header set** (len passed in-process) | digest via `tracing::info!` :175-178; latency truncate :186-191; timer **no `delay=0`** :204 | [HEADER-ASYM] (see F1); timer note |
| camel-standalone-yaml | B | B: 10ms tick | same 8 bean steps, s-routes.yaml:23-52; beans AppYaml.java:89-185 (assert :120-152 incl. readTree :131, header :150, marker :158-165, counter+append :171-185) | same as dsl cell (:56-80); timer `delay=0` (s-routes.yaml:29) | — |
| camel-quarkus-yaml-native | B | B: 10ms tick | same via BenchBeans.java:58-118 (assert :154-186, header :184, marker :192-199, append :205-219); qy-routes.yaml:30-59 | same (:69-118); timer `delay=0` (qy-routes.yaml:36) | — |
| rust-camel-cli | B | B: 10ms tick | cache lookup per tick (cli.yaml:74-79; first tick builds body via rhai + rhai validation :82-105); unmarshal :108; jsonpath `$.id` filter :109-111; **js transform** :113-115; marshal :117; rhai assert (len + contains only) :119-127; set_header via rhai :128-130; idempotent-repo marker gate + log :141-145; **no route latency step** — CLI runtime module (route mode) appends `BENCH_LATENCY` (harness `run.sh:1788-1793`; `bench_instrument.rs:24-39`) | `BENCH_INPUT_SHA256=GOLDEN` literal :106; timer `delay=0` :66 | [LOG-ASYM] F2; [COUNTER-ASYM] F3; [BODY-ASYM] F4, F5; [WRAPPER-ASYM] F6 |
| node-native | node | B: 10ms tick | full pipeline per tick: JSON.parse :171, insert :180, stringify :183, full assert :137-162 (incl. second parse :144); marker latch :193-200; `appendFileSync` per tick :245; window t0 before pipeline :235 | body build once + digest + truncate :202-227; immediate first fire :254 | — (family-internal symmetric with fastify) |
| node-fastify | node | B: 10ms tick | identical contract (pipeline :140-158, loop :216-232) + fastify boot (no bind) before startup work :179-189 | same + avvio boot | — |

**Flags (t2-json):**
- **F1 [HEADER-ASYM]** (Pair A): the JVM cells set a `benchOutLen` message header per tick (App.java:188; BenchRoute.java:167; AppYaml.java:150; BenchBeans.java:184); rust-camel-lib sets no header (t2-json.rs:233-239 passes the length in-process). Pair B internally: standalone-yaml/quarkus-yaml set it, cli sets it via a rhai evaluation (cli.yaml:128-130) — the rhai eval is the pair-B cost of the same header. Cross-pair header presence is design-equal; the flag is the lib-vs-Java divergence inside Pair A.
- **F2 [LOG-ASYM]** (cli): the provenance line is the literal `BENCH_INPUT_SHA256=GOLDEN` (cli.yaml:106) instead of the real digest every other cell computes (App.java:82; t2json.rs:175-178; node-native t2-json.mjs:223). Startup-only line; not per-tick.
- **F3 [COUNTER-ASYM]** (cli): the per-tick marker gate is an idempotent-consumer memory-repo probe (cli.yaml:141-143) where Java uses an AtomicBoolean CAS (AppYaml.java:160) and lib an AtomicBool swap (t2json.rs:237).
- **F4 [BODY-ASYM]** (cli): output assert is len + `contains` only (cli.yaml:119-127); every other cell re-parses and checks id/seq/fill-all-'b'/bench (App.java:169-187; t2json.rs:131-150; node-native t2-json.mjs:144-161). The cli assert cannot catch a fill-corruption or seq regression.
- **F5 [BODY-ASYM]** (cli): body supply runs through a cache step with first-tick rhai construction (cli.yaml:74-90); all other cells `setBody` a prebuilt constant each tick (App.java:104 area; t2json.rs:206).
- **F6 [WRAPPER-ASYM]** (cli): the `BENCH_LATENCY` record comes from the CLI runtime module in route mode — window = route entry → last step (`bench_instrument.rs:24-39`), which INCLUDES set_body/cache and the marker gate, while the Java stamp sits after setBody (App.java:104-105) and the lib stamp after set_body (t2json.rs:211). The module also writes via an open-once `Mutex<File>` (`bench_instrument.rs:16-18`) vs per-tick reopen in Java/lib/node — the reopen cost lands outside every window, so this is background-cost, not window, asymmetry.
- Note: lib timer lacks `delay=0` (t2json.rs:204) — first tick fires ~1s later than the seven peers; per-tick work unaffected, time-to-first-record extended.
- Node family internal: native and fastify are contract-identical; fastify adds the avvio boot (its cell's purpose).

## Scenario 3 — t2-realistic-eip (T2, protocol B; 10ms tick × 10000)

Per-tick pipeline: stamp start → setBody("ping") → setHeader(source=bench) → filter(simple `body=='ping'`) → choice(when `header.source=='bench'` → "pong-bench" / otherwise "pong-other") → marker once (`BENCH_ROUTE_READY body=<body>`) → `BENCH_LATENCY` append. Latency windows are ALIGNED in this scenario: every cell stamps before set_body. Marker suffix `body=` is uniform except lib's double line (F1).

File key: `App.java` = `scenarios/t2-realistic-eip/camel-standalone/camel-standalone-dsl/.../App.java`; `AppYaml.java` = `camel-standalone-yaml/.../AppYaml.java`; `s-routes.yaml` = `camel-standalone-yaml/src/main/resources/routes.yaml`; `BenchRoute.java` = `camel-quarkus/camel-quarkus-dsl/.../BenchRoute.java`; `LatencyBean.java` = `camel-quarkus/camel-quarkus-yaml/.../LatencyBean.java`; `qy-routes.yaml` = `camel-quarkus/camel-quarkus-yaml/src/main/resources/camel/routes.yaml`; `t2r.rs` = `contenders/rust-camel-lib/src/scenarios/t2-realistic-eip.rs`; `cli.yaml` = `scenarios/t2-realistic-eip/rust-camel-cli/routes/t2-realistic-eip.yaml`; node = `contenders/node/node-{native,fastify}/t2-realistic-eip.mjs`.

| Contender | Pair | Protocol | Work per tick | Work per process startup | Flags |
|---|---|---|---|---|---|
| camel-standalone-dsl | A | B: 10ms tick | stamp :69-70 → setBody :71 → setHeader :72 → simple filter :73 → choice :74-80 → marker CAS + println :86-91 → counter + direct append :92-105 | latency truncate :52-55; timer `delay=0` :63 | — |
| camel-quarkus-dsl-native | A | B: 10ms tick | identical shape (BenchRoute.java:51-85; marker :76, counter+append :81-85); timer `delay=0` :51 | same pattern | — |
| rust-camel-lib | A | B: 10ms tick | stamp :135-138 (before set_body) → set_body :139 → set_header :140 → closure filter :144 → choice/when :154-166 → marker :174-184 — **emits TWO lines on first tick** (`BENCH_ROUTE_READY` :178 and `BENCH_ROUTE_READY body=…` :180) → counter + append :185-200 | latency truncate :111-116; timer **no `delay=0`** :129 | [LOG-ASYM] F1; timer note |
| camel-standalone-yaml | B | B: 10ms tick | markStart → setBody → setHeader → filter → choice, then emitMarker/writeLatency beans (s-routes.yaml:29-60; AppYaml.java:58-60, 70-105) | truncate :49-51; timer `delay=0` (s-routes.yaml:35) | — |
| camel-quarkus-yaml-native | B | B: 10ms tick | same bean shape via LatencyBean.java:52-121; qy-routes.yaml:21-52 | same (:66-79); timer `delay=0` (qy-routes.yaml:27) | — |
| rust-camel-cli | B | B: 10ms tick | set_body :29-30 → set_header :31-33 → simple filter :34-36 → choice :37-45 → idempotent gate + marker log `body=${body}` :53-57; module route-mode latency (harness `run.sh:1788-1793`) | timer `delay=0` :27 | [COUNTER-ASYM] F2 |
| node-native | node | B: 10ms tick | synthetic exchange :98-100; set/filter/choice as plain JS :102-122; `logStep` latched to first tick :87-92 (no per-tick stdout); loop :139-149 | truncate :134-135; immediate first fire :154 | — |
| node-fastify | node | B: 10ms tick | same contract + fastify boot (no bind) | same | — |

**Flags (t2-realistic-eip):**
- **F1 [LOG-ASYM]** (Pair A): lib prints two marker lines on the first tick (t2r.rs:177-181) where all seven peers print one (App.java:86-91; BenchRoute.java:76; cli.yaml:57; node-native t2-realistic-eip.mjs:87-92). One-time, not per-tick.
- **F2 [COUNTER-ASYM]** (cli): idempotent-repo probe per tick as the marker gate (cli.yaml:53-55) vs CAS/swap in the six framework cells.
- Note: lib timer lacks `delay=0` (t2r.rs:129) — same first-tick lag as t2-json.
- Latency windows: no flag — all cells bracket from before set_body (Java stamp is step 1, s-routes.yaml:37-38; App.java:69-70 before :71; lib :135-138 before :139; node t0 :142; cli route entry).

## Scenario 4 — split-aggregate (T2s, protocol B; 10ms tick × 10000)

Per tick: set body (591-byte canonical 100-item array) → tick-start store → split (sequential) → per fragment `direct:agg-in` → aggregate (`completionSize=100`, list-append) → completion assert → marker once (`BENCH_ROUTE_READY items=<n>`). Marker format uniform.

File key: `App.java` = `scenarios/split-aggregate/camel-standalone/camel-standalone-dsl/.../App.java`; `s-routes.yaml` = `camel-standalone-yaml/src/main/resources/routes.yaml`; `AppYaml.java` = `camel-standalone-yaml/.../AppYaml.java`; `Strategy.java` = `camel-standalone-yaml/.../ListAppendStrategy.java`; `BenchRoute.java` = `camel-quarkus/camel-quarkus-dsl/.../BenchRoute.java`; `BenchBeans.java` = `camel-quarkus/camel-quarkus-yaml/.../BenchBeans.java`; `qy-routes.yaml` = `camel-quarkus/camel-quarkus-yaml/src/main/resources/camel/routes.yaml`; `split.rs` = `contenders/rust-camel-lib/src/scenarios/split-aggregate.rs`; `cli.yaml` = `scenarios/split-aggregate/rust-camel-cli/routes/split-aggregate.yaml`; node = `contenders/node/node-{native,fastify}/split-aggregate.mjs`.

| Contender | Pair | Protocol | Work per tick | Work per process startup | Flags |
|---|---|---|---|---|---|
| camel-standalone-dsl | A | B: 10ms tick | setBody(constant array) :108 → tickStartNanos atomic store :109-110 → split(jsonpath `$`) :111 → 100× `direct:agg-in` :112 → latency write :114-125; agg route: setHeader :139 → aggregate w/ appendToList :140-141 (ArrayList copy per fragment :158-168, O(n²)/tick) → assert (CamelAggregatedSize + list size) :142/:174-190 → marker CAS :143-148 | SHA line :81; truncate :85-88; timer `delay=0` :107 | — |
| camel-quarkus-dsl-native | A | B: 10ms tick | identical (BenchRoute.java:90-127; appendToList :137-146; assert :153+); timer `delay=0` :90 | same | — |
| rust-camel-lib | A | B: 10ms tick | set_body :314 → SystemTime store :330-340 → unmarshal :346 → split parallel(false) :347 → to direct :348 → latency :350-367 (wall-clock `SystemTime` diff :356-360; per-tick reopen :362); agg route :161-177: set_header :166-169 → aggregate CollectAll :170 (config :146-153) → completion_assert :193-236 (pending skip :194-200, len + CamelAggregatedSize consistency) → marker :244-254 | SHA :277; truncate :285-290; timer **no `delay=0`** :312 | timer note; clock note |
| camel-standalone-yaml | B | B: 10ms tick | buildArray (prebuilt constant, AppYaml.java:78, 90-93) → markStart → split jsonpath (s-routes.yaml:40-61) → writeLatency; agg route: setHeader → `#class:` ListAppendStrategy (Strategy.java:26-34, same copy+add) → completionSize 100 → assert → emitMarker (s-routes.yaml:59-81) | same; timer `delay=0` (s-routes.yaml:46) | — |
| camel-quarkus-yaml-native | B | B: 10ms tick | same bean shape (BenchBeans.java:66-86; qy-routes.yaml:46-85) | same | — |
| rust-camel-cli | B | B: 10ms tick | set_body literal array :67-69 → rhai len==591 :71-77 → idempotent-gated real SHA log :82-86 → unmarshal :88 → split `json_array` :91-94 → direct; agg route: set_header :99-101 → aggregate collect_all :102-107 → rhai filter `property==100` :111-113 → js assert length :116-122 → set_property 100 (hardcoded) :123-125 → idempotent marker log :133-137; module route-mode latency | timer `delay=0` :59 | [COUNTER-ASYM] F1; [BODY-ASYM] F2; [WRAPPER-ASYM] F3 |
| node-native | node | B: 10ms tick | JSON.parse → sequential fragment loop → `directAggIn` (header set :137, bucket push :145, completion reset :161-166) → completionAssert (pending skip + consistency, mirroring lib :174-194) → marker latched | array built once :113-116 | — |
| node-fastify | node | B: 10ms tick | same contract + fastify boot (:173+) | same | — |

**Flags (split-aggregate):**
- **F1 [COUNTER-ASYM]** (cli): two idempotent-repo probes per tick (sha-once + marker-once, cli.yaml:82-84 and :133-135) plus the rhai property gate (:111-112), vs a single CAS/swap (App.java:143-147; split.rs:249) or plain counter elsewhere.
- **F2 [BODY-ASYM]** (cli): completion assert checks `body.length==100` via js and then hardcodes `bench.aggregated.size = 100` (cli.yaml:116-125); the JVM cells cross-check `CamelAggregatedSize` against the collected list (App.java:174-190) and lib checks reported-vs-actual consistency (split.rs:220-232). Practical gate equivalent (100 is pinned by the body), but the consistency check is weaker.
- **F3 [WRAPPER-ASYM]** (cli): module route-mode window starts at route entry, including set_body + the rhai len check + the idempotent sha gate; Java stores tickStart AFTER setBody (App.java:109-110) and lib after set_body (split.rs:330-340) — cli window is wider by the body-supply and input-validation steps.
- Notes (untagged): (a) lib's split window clock is non-monotonic `SystemTime` (split.rs:333-337, 356-360) vs `System.nanoTime` (App.java:110,116) and the module's `Instant` (`bench_instrument.rs:57`) — NTP-step exposure, measurement quality only. (b) Aggregation strategy mechanics differ by runtime (Java copy-per-fragment list, App.java:158-168/Strategy.java:27-33; lib/cli CollectAll split.rs:149, cli.yaml:105; node bucket push split-aggregate.mjs:145) — inherent EIP implementation cost, not fixture unfairness. (c) lib timer lacks `delay=0` (split.rs:312).

## Scenario 5 — http-server (T3, protocol A: http request/response)

Per-request contract: any-method POST `/bench` → 200 + `pong` (text/plain). The per-request work rows below are the whole story of this scenario's fairness problem.

File key: `App.java` = `scenarios/http-server/camel-standalone/camel-standalone-dsl/.../App.java`; `AppYaml.java` = `camel-standalone-yaml/.../AppYaml.java`; `s-routes.yaml` = `camel-standalone-yaml/src/main/resources/routes.yaml`; `BenchRoute.java` = `camel-quarkus/camel-quarkus-dsl/.../BenchRoute.java`; `NativeBenchRoute.java` = `camel-quarkus/camel-quarkus-dsl-native/.../NativeBenchRoute.java`; `Marker.java` = `camel-quarkus/camel-quarkus-yaml/.../RouteStartedMarker.java`; `qy-routes.yaml` = `camel-quarkus/camel-quarkus-yaml/src/main/resources/camel/routes.yaml`; `http.rs` = `contenders/rust-camel-lib/src/scenarios/http-server.rs`; `cli.yaml` = `scenarios/http-server/rust-camel-cli/routes/http-server.yaml`; `wrapper.sh` = same dir `http-server-cli-wrapper.sh`; node = `contenders/node/node-{native,fastify}/http-server.mjs`; `axum.rs` = `contenders/axum-bare/src/main.rs`.

| Contender | Pair | Protocol | Work per request | Work per process startup | Flags |
|---|---|---|---|---|---|
| camel-standalone-dsl | A | A: http req | NONE — `from(jetty).setBody(constant("pong"))` (App.java:122-123); docstring states no log/counter/process step :38-43 | JVM boot; marker via RouteStarted event notifier :67-97 | — (code matches docstring) |
| camel-quarkus-dsl-native | A | A: http req | NONE — `from(platform-http:/bench).setBody(constant("pong"))` (NativeBenchRoute.java:99-100) | Quarkus native boot; RouteStarted marker :59-85 | [BODY-ASYM] F1 (transport divergence vs JVM sibling) |
| rust-camel-lib | A | A: http req | `.log("BENCH_HTTP_REQUEST received")` :101 + `.process` (AtomicU64 `fetch_add` + `tracing::info!("…id={id}")`) :102-109 + `.set_body("pong")` :110 → **2 stdout lines + 1 atomic + body set** | tracing init :74; direct `println!` marker after `ctx.start()` :143-147 | [LOG-ASYM] F2; [COUNTER-ASYM] F3 |
| camel-standalone-yaml | B | A: http req | NONE — bare `setBody: constant: "pong"` (s-routes.yaml:21-26) | JVM boot + YAML parse; RouteStarted marker (AppYaml.java:50-80) | — |
| camel-quarkus-yaml-native | B | A: http req | NONE — bare setBody (qy-routes.yaml:23-28) | Quarkus native boot; RouteStartedMarker notifier (Marker.java:84-108) | — |
| rust-camel-cli | B | A: http req | NONE — bare `from` + `set_body: pong` (cli.yaml:30-35) | CLI boot + YAML parse; marker emitted by wrapper on child's `CamelContext started` (wrapper.sh:89, 220-228) | — (wrapper startup-shape noted in F4) |
| node-native | node | A: http req | counter `requestId += 1` :48 + `console.log received` :49 + `console.log id=<n>` :50 + explicit `writeHead(200, CT+Content-Length)` :54-57 + `res.end("pong")` :58 → **2 stdout lines + counter** (bench-path only; 404s consume nothing :40-47) | latency-file touch :33-35; marker from listen callback :61-63 | — (family-internal symmetric) |
| node-fastify | node | A: http req | same 2 lines + counter (:42-44) + `return "pong"` :45 | fastify boot + catch-all content-type parser :26-34; marker :48-49 | [BODY-ASYM] F5 (within family) |
| axum-bare (reference) | ref | A: http req | `println received` :87 + body drain `to_bytes(≤1MiB)` :89 + atomic fetch_add :90 + `println id=<n>` :91 + respond :92-96 | TcpListener bind + spawn; bare `println!("BENCH_ROUTE_READY")` **without unix_ms** :63-66 | reference deviations, not counted: drain (axum.rs:33-34,89); marker suffix absent |

**Flags (http-server):**
- **F1 [BODY-ASYM]** (Pair A internal): the two quarkus-native-vs-JVM siblings serve over different HTTP stacks — jetty in the JVM cell (BenchRoute.java:96) vs platform-http/Vert.x in the native cell (NativeBenchRoute.java:99). Deliberate and documented (NativeBenchRoute.java:16-19; BenchRoute.java:26-30 — "platform-http … +18.8% faster in native mode"), but it means the Pair A jetty-vs-platform-http axis is confounded with the JVM-vs-native axis inside the pair.
- **F2 [LOG-ASYM]** (Pair A): rust-camel-lib emits 2 stdout lines per request (http.rs:101-106); both Pair A Java cells emit zero. Pair B is clean (all zero). This is the rc-am22 restore diverging from the fixture family — see [Era-2 validation](#era-2-validation) and [Contradictions](#contradictions-found) C1.
- **F3 [COUNTER-ASYM]** (Pair A): lib increments an AtomicU64 per request (http.rs:105); no Java cell has any per-request counter. (Node family: both cells count; axum reference counts.)
- **F4 [WRAPPER-ASYM]** (cli): the wrapper spawns `setsid camel run` plus two `tail -F` forwarders and polls the child stdout file at 10ms for the ready line (wrapper.sh:181-209, 213-251) — a 3-process startup shape vs direct exec for every other cell, and the M1 marker stops at wrapper detection time. Per-request work is pass-through only (no extraction, no mutation). This startup-shape asymmetry is already caveated in the era-2 record for M1.
- **F5 [BODY-ASYM]** (node family internal): fastify registers a catch-all content-type parser with `parseAs: "string"` (http-server.mjs:32-34) so the request body is buffered to a string per request; native never touches the body. Both ignore its content.
- Metrics lever variants (`Camel.toml.metrics-on` / `.metrics-off`) exist but are NOT wired into the harness default: the harness references only `Camel.toml` (`run.sh:1463, 1687`); the lever path is opt-in via `BENCH_CAMEL_TOML` wrapper passthrough (wrapper.sh:124-129). The metrics-on arm would add per-request Prometheus emission (exchange counter + duration histogram + component ops); the base fixture has no observability block, so no per-request metric work at default.
- **Smoke contract answer** (see [Open questions](#open-questions-for-the-canonical-shape-decision) Q1 for the full consequence): `smoke/run.sh` asserts `BENCH_HTTP_REQUEST id=1` on the 9 non-cli smoke artifacts (6 Java + lib + 2 node; axum checked inline); rust-camel-cli gets a partial pass that only checks for the static `received` line (run.sh:178-198; cli branch :185-191; the script's own "7/8" header comment predates the node axis and the quarkus JVM siblings). The four Java cells' fixtures at HEAD emit no id line — they pass today only because the committed `smoke/*.log` evidence is stale (Java + cli logs carry 2026-07-19 timestamps and the cli log references the old `rc-f3g9-startup-benchmark` worktree; the lib log is 2026-09-11 and node logs 2026-09-01). A fresh smoke run would hard-fail the six Java cells on `id=1` (run.sh:194-197) and soft-WARN the cli (:186-188).

## Scenario 6 — xsd-validation-bridge (T4b, protocol B; 10ms tick × 10000)

Bridge design (not an asymmetry): the Java cells run Xerces-J validation in-process; the rust cells delegate the same validation to `bridges/xml` via gRPC mTLS, paying the bridge tax (standalone App.java docstring; cli yaml comment). Pair-internal comparisons are the valid axis. All markers are `BENCH_ROUTE_READY <unix_ms>`.

File key: `App.java` = `scenarios/xsd-validation-bridge/camel-standalone/camel-standalone-dsl/.../App.java`; `BenchRoute.java` = `camel-quarkus/camel-quarkus-dsl/.../BenchRoute.java`; `xsd.rs` = `contenders/rust-camel-lib/src/scenarios/xsd-validation-bridge.rs`; `cli.yaml` = `scenarios/xsd-validation-bridge/rust-camel-cli/routes/xsd-bench.yaml`; `wrapper.sh` = same dir `xsd-validation-bridge-cli-wrapper.sh`; `pid-wrapper.sh` = `shared/bridge-wrapper.sh`; node = `contenders/node/node-{native,fastify}/xsd-validation-bridge.mjs`.

| Contender | Pair | Protocol | Work per tick | Work per process startup | Flags |
|---|---|---|---|---|---|
| camel-standalone-dsl | A | B: 10ms tick | setBody(payload) :88 → StreamSource wrap :98-103 → stamp :107-109 → `to(validator:file://<abs schema>)` :110 → counter + direct append :111-117; **no per-tick log step** (verified by grep) | payload/schema read; truncate; RouteStarted marker :58-71; timer no `delay=0` :86 | — |
| camel-quarkus-dsl-native | A | B: 10ms tick | setBody :84 → wrap :88-91 → stamp :97 → validator :99 → counter + append :101-107 → **per-tick log WITH timer-counter id** :114 | same; RouteStarted marker :60-70; timer no `delay=0` :82 | [LOG-ASYM] F1 |
| rust-camel-lib | A | B: 10ms tick | set_body + stamp :91-101 (stamp :97-100) → `to(validator:<path>)` :101 (gRPC bridge subprocess) → counter + append :102-117 (reopen :112-113) → bare log `BENCH_XSD_TICK` :118 | payload via `BENCH_PAYLOAD` env :54-67; bridge env wiring :74-79; marker `println!` after start :124-128; timer no `delay=0` :91 | — (bare-log format noted) |
| rust-camel-cli | B | B: 10ms tick | set_body (inline payload) :34-77 → `to(validator:shared/schema.xsd)` RELATIVE :80 → bare log :86; latency written by the CLI runtime module in **route mode** (wrapper exports `BENCH_LATENCY_MODE=route`, wrapper.sh:171,177) | wrapper: truncates latency file :95, bridge env :104-119, cd to scenario dir :183, setsid spawn + 2 tails :185-207, marker poll on `CamelContext started` :213-239; timer `delay=0` :29 | [WRAPPER-ASYM] F2, F3 |
| node-native | node | B: 10ms tick | async validation via **xmllint-wasm (libxml2)** worker :89-185 → `appendFileSync` :228 → per-tick log WITH tick id :229 | payload/schema read :66-67; truncate :71-72; worker spawn; self-test :199-208; marker :244 BEFORE first tick (first tick at +10ms :243) | family caveat: engine is libxml2, not Xerces-J |
| node-fastify | node | B: 10ms tick | same + fastify boot (diff: import + `app.all` + `await app.ready()`) | same + boot; different default latency path | — |

**Flags (xsd-validation-bridge):**
- **F1 [LOG-ASYM]** (Pair A): standalone-dsl emits zero per-tick stdout lines while quarkus-dsl-native emits one WITH a `CamelTimerCounter` interpolation (BenchRoute.java:114). lib emits one bare line (:118); cli one bare line (cli.yaml:86); node one with tick id (:229). Same-check: within Pair B there is only the cli cell, so the cross-pair format spread is informational.
- **F2 [WRAPPER-ASYM]** (cli): route-mode module brackets route entry → last step, so the cli per-tick latency INCLUDES set_body AND the trailing log step (cli.yaml:34-86); the Java cells stamp immediately before the validator call and close at the latency step, excluding their trailing log (App.java:107-110; BenchRoute.java:97-99) and lib does the same (xsd.rs:97-101, log after window :118).
- **F3 [WRAPPER-ASYM]** (cli): the validator URI is relative (`validator:shared/schema.xsd`, cli.yaml:80) and only resolves because the wrapper cd's to the scenario dir (wrapper.sh:183); Java peers use absolute `file://` URIs (App.java:110; BenchRoute.java:99) and lib the `BENCH_SCHEMA` env path (xsd.rs:56-57,90). Same schema file, different resolution mechanics (cwd-coupled).
- Notes: cli `delay=0` (:29) vs none in Java/lib; first BENCH_LATENCY therefore arrives earlier for cli. Bridge PID bookkeeping exists only for the rust cells (pid-wrapper.sh:55-63; wrapper.sh:96-97,118-119; harness `run.sh:1437-1439`) — pair design. Contradiction C3 (harness comment claims pair mode) applies to this cell.

## Scenario 7 — xslt-bridge (T4a, protocol B; 10ms tick × 10000)

Bridge design: Java cells transform in-process with Saxon-HE; rust cells delegate to the gRPC xml bridge (Saxon-HE in the bridge). All markers `BENCH_ROUTE_READY <unix_ms>`. The cli wrapper is line-identical to the xsd wrapper except names/paths/latency default (verified by diff).

File key: `App.java` = `scenarios/xslt-bridge/camel-standalone/camel-standalone-dsl/.../App.java`; `pom.xml` = same module `pom.xml`; `BenchRoute.java` = `camel-quarkus/camel-quarkus-dsl/.../BenchRoute.java`; `xslt.rs` = `contenders/rust-camel-lib/src/scenarios/xslt-bridge.rs`; `cli.yaml` = `scenarios/xslt-bridge/rust-camel-cli/routes/xslt-bench.yaml`; `wrapper.sh` = same dir `xslt-bridge-cli-wrapper.sh`; node = `contenders/node/node-{native,fastify}/xslt-bridge.mjs`.

| Contender | Pair | Protocol | Work per tick | Work per process startup | Flags |
|---|---|---|---|---|---|
| camel-standalone-dsl | A | B: 10ms tick | setBody :123 → StreamSource wrap :135-138 → stamp :144-146 → `to(xslt:file://<abs stylesheet>)` :147 (camel-xslt endpoint, Saxon-HE + pinned Xerces in pom.xml:65,89-96) → counter + append :148-155 → **log WITH `CamelTimerCounter` id** :171 | payload prebuilt; RouteStarted marker :92-106; timer `period=10ms` :121 | — |
| camel-quarkus-dsl-native | A | B: 10ms tick | setBody :229 → stamp :235-236 → **inline Saxon process step**: cached `Templates` + `newTransformer()` per tick + transform + setBody :261-268 (Templates mirror of bridge service :53-54, :116-156) → counter + append :270-275 → log WITH id :284 | RouteStarted marker :215 area; timer `period=10ms` :227 | [BODY-ASYM] F1 |
| rust-camel-lib | A | B: 10ms tick | set_body + stamp :145-153 → `to(xslt:<path>)` :154 (gRPC bridge) → counter + append :155-173 → bare log :174 | `BENCH_PAYLOAD`/`BENCH_STYLESHEET` env :85-90; bridge wiring :121-128; marker :185-189; timer `period=10` (=`10ms`) :143 | — |
| rust-camel-cli | B | B: 10ms tick | set_body (inline payload) → `to(xslt:shared/identity-transform.xsl)` RELATIVE :83 → bare log :90; route-mode module latency (wrapper.sh:171,177) | wrapper identical to xsd wrapper modulo names (verified via diff); timer `delay=0` :35 | [WRAPPER-ASYM] F2, F3 |
| node-native | node | B: 10ms tick | **Saxon-JS** transform (:66,112; stylesheet pre-compiled by xslt3 CLI at startup :98-101) → append :148 → log WITH tick id :149 | xslt3 compile + self-test + `BENCH_XSLT_SELFTEST_SHA256` line :122-130; marker after scheduling :157+ | family caveat: Saxon-JS ≠ Saxon-HE (documented in-file :2-4) |
| node-fastify | node | B: 10ms tick | same + fastify boot | same | — |

**Flags (xslt-bridge):**
- **F1 [BODY-ASYM]** (Pair A internal): the two Java cells reach Saxon through different mechanics — standalone-dsl via the camel-xslt endpoint (App.java:147), quarkus-dsl-native via a hand-rolled in-route processor with cached Templates (BenchRoute.java:261-268). Same engine family, different invocation path; the native shape is documented as mirroring `XsltTransformerService` to match the bridge's compilation behavior (:53-54).
- **F2 [WRAPPER-ASYM]** (cli): same route-mode window inclusion as xsd — the cli per-tick record includes set_body and the trailing log step; the Java/lib windows exclude theirs (App.java:144-147; BenchRoute.java:235-239; xslt.rs:150-154).
- **F3 [WRAPPER-ASYM]** (cli): relative stylesheet URI (`xslt:shared/identity-transform.xsl`, cli.yaml:83) resolved via the wrapper's cd (wrapper.sh:183) vs absolute `file://` (App.java:147) and env-path (xslt.rs:87-88,142) in the peers.
- Notes: per-tick log id interpolation is present in both JVM cells and node, absent in lib/cli — format spread, trivial cost. `period=10ms` (JVM) ≡ `period=10` (rust) — same duration. cli `delay=0` vs none in Java/lib; node first tick at +10ms.

## Summary matrix of flags

Counts are within-scenario flags per the taxonomy (Pair A/B internal or scenario-wide). Node-family INTERNAL flags count on the family axis (fastify-vs-native divergences, e.g. F5 in http-server); node-vs-Camel engine caveats (xmllint-wasm vs Xerces-J, Saxon-JS vs Saxon-HE) and all axum-reference deviations are notes only, never counted. `[PARSE-ASYM]` and `[PROTOCOL]`: zero occurrences — no within-pair parse divergence and no unit mixing were found.

| Scenario | LOG-ASYM | COUNTER-ASYM | BODY-ASYM | HEADER-ASYM | WRAPPER-ASYM | Total |
|---|---|---|---|---|---|---|
| startup-minimal | 0 | 0 | 0 | 0 | 2 | **2** |
| t2-json | 1 | 1 | 2 | 1 | 1 | **6** |
| t2-realistic-eip | 1 | 1 | 0 | 0 | 0 | **2** |
| split-aggregate | 0 | 1 | 1 | 0 | 1 | **3** |
| http-server | 1 | 1 | 2 | 0 | 1 | **5** |
| xsd-validation-bridge | 1 | 0 | 0 | 0 | 2 | **3** |
| xslt-bridge | 0 | 0 | 1 | 0 | 2 | **3** |
| **Total** | **4** | **4** | **6** | **1** | **9** | **24** |

The dominant pattern: `rust-camel-cli` carries a [WRAPPER-ASYM] in 4 of its 5 protocol-B scenarios (not t2-realistic-eip, where the latency windows align — see that scenario's note), and `rust-camel-lib` is the fixture that diverged from its family in http-server (log + counter per request) and in the three warm-tick scenarios' timer URI (`delay=0` missing).

## Contradictions found

- **C1 — `http-server.rs` docstring vs every Java cell and the cli route.** `contenders/rust-camel-lib/src/scenarios/http-server.rs:38-45` claims: "Every other http-server contender emits `BENCH_HTTP_REQUEST received` plus a per-request `BENCH_HTTP_REQUEST id=<n>` line, and the smoke … asserts `id=1` on each artifact." FALSE at HEAD: the 4 Java fixtures are bare `setBody("pong")` (App.java:122-123; NativeBenchRoute.java:99-100; s-routes.yaml:21-26; qy-routes.yaml:23-28) and the cli route is bare (cli.yaml:30-35). Only node-native, node-fastify, and axum-bare emit the two lines. The same docstring's "smoke asserts id=1 on each artifact" is also imprecise: the smoke exempts rust-camel-cli (run.sh:185-191).
- **C2 — smoke script vs committed smoke evidence.** `smoke/run.sh:173-198` still mandates `id=<n>` on the 9 non-cli smoke artifacts (the script's own "7/8" header comment predates the node axis and the quarkus JVM siblings), but the committed logs that show Java cells passing (`camel-standalone-dsl.log` ends with `BENCH_HTTP_REQUEST received` + `id=1`) carry 2026-07-19 timestamps, and `rust-camel-cli.log` references the deleted `.worktrees/rc-f3g9-startup-benchmark` path — i.e., captured against the pre-"canonical minimal" fixtures. Current code cannot produce those lines. The green evidence is stale; a re-run fails 6 Java cells and warns on cli.
- **C3 — harness comment vs wrapper code (bridge cli latency mode).** `run.sh:1785-1786` states "The xsd-validation-bridge cli cell sets NO mode and keeps the default pair-mode wrapping", but `xsd-validation-bridge-cli-wrapper.sh:177` exports `BENCH_LATENCY_MODE=route` (and `xslt-bridge-cli-wrapper.sh:177` equivalently). The bridge cli cells run ROUTE mode, wrapping the whole timer route — which also changes the measured window (see F2/F3 in both bridge scenarios). Code wins.
- **C4 — harness comment vs wrapper code (latency extraction).** `run.sh:1443-1445` says the bridge cli wrapper "extracts BENCH_LATENCY from child stdout + writes the PID file". The wrapper does neither extraction nor latency writing: it exports `BENCH_LATENCY_FILE` for the child (wrapper.sh:171) and the CLI runtime module appends records directly to the file (`bench_instrument.rs:90-103`). The only stdout handling is marker detection and pass-through tailing (wrapper.sh:213-239, 195-207). No wrapper at HEAD performs per-tick stdout extraction.
- **C5 — `http-server/Camel.toml` header vs route file.** `Camel.toml:9-11` describes the route shape as `from(...) -> log -> set_body(pong)`, but `routes/http-server.yaml:30-35` contains only `from` + `set_body` — no log step. The comment describes the pre-rc-5gcu/route-edit era.
- **C6 — era-2 self-description (folded into C1).** The rc-am22 restore restored the smoke-trace steps only into the lib fixture while its docstring asserts family-wide symmetry that does not exist; the era-2 shape of the same file (9e8f36f) was genuinely bare (see next section).

## Era-2 validation

Verified directly against commit `9e8f36f`:

- `benchmarks/contenders/rust-camel-lib/src/scenarios/http-server.rs` at `9e8f36f`: docstring "no per-request counter, no process step, no log emission" (lines 38, 81-82) and route consisting of `from(...)` + `.set_body("pong")` (line 89). No log step, no process step, no counter.
- `benchmarks/scenarios/http-server/rust-camel-cli/routes/http-server.yaml` at `9e8f36f`: same bare shape as today — `from:` (line 32) + `set_body:` (line 34), nothing else.

**Verdict:** era-2 lib-vs-cli was symmetric-bare on the fixture axis, so the "+23.3% cli vs era-2" comparison is like-for-like fixture-wise and **STANDS**. The current HEAD breaks that symmetry in the other direction: the lib cell gained `log` + `process(counter)` per request (http-server.rs:101-110) while the cli cell stayed bare — current lib-vs-cli numbers are NOT fixture-comparable, and the divergence is lib's (it no longer matches its own era-2 shape), not cli's.

## Open questions for the canonical-shape decision

1. **The smoke `id=1` dependency (answered for the audit, open for policy):** yes, `smoke/run.sh` asserts `BENCH_HTTP_REQUEST id=1` on the 9 non-cli smoke artifacts (all but cli, plus axum checked inline; run.sh:178-198 — the script's "7/8" comment is stale), and the bare Java cells pass only via stale July-era logs. Either (a) every http-server fixture restores the `received` + `id=<n>` pair — making per-request lines the declared artifact contract (then the M3/M4 throughput cells must count 2 stdout writes per request for everyone, and lib stops being the outlier) — or (b) `verify_request_id` becomes WARN-only. The current state is latent red and the lib docstring (C1) is the only place asserting the contract that no longer exists.
2. **Should per-request stdout lines exist anywhere during measurement?** Node family and axum-bare pay 2 lines/request; the measured Camel cells pay 0 (except lib). If M3/M4 saturation runs ever admit the node family, this [LOG-ASYM] becomes a cross-family confound at tens of thousands of requests per second.
3. **Window-boundary parity for route-mode:** the cli module measures route-entry→last-step (including body supply and a trailing log step in the bridges) while Java/lib stamp immediately before the bridge/transform call. If canonical shape wants strict comparability, either reposition the cli stamp (pair-mode-style anchoring) or accept and document the wider window.
4. **Assert-strength canonicalization in t2-json:** the cli's len+contains assert cannot catch fill/seq corruption that the other six cells' full semantic re-parse catches. Which assert is canonical?
5. **lib timer `delay=0` gap** (t2json.rs:204, t2r.rs:129, split.rs:312): ~1s to first record vs peers. Harmless to per-tick latency, but it inflates M2 probe wall-clock and diverges from the URI every peer uses.
6. **Marker format:** `BENCH_ROUTE_READY` with `<unix_ms>` suffix everywhere except axum-bare (axum.rs:63) and except startup-minimal (no suffix anywhere, by design). Harmless under `grep -F` today; worth pinning if any consumer ever parses the suffix.
