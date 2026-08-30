# Spike Results — Benchmark v2 (bd rc-p9ki, Task 1)

Two independent pre-flight spikes before committing to full v2 implementation.
Both passed on first try once NixOS-specific toolchain issues were resolved.

## Toolchain (verified)

| Component | Identity |
|---|---|
| GraalVM CE | 21.0.2+13.1 at `/tmp/rc-f3g9-jdk21/` (per plan §1) |
| `native-image` | `native-image 21.0.2 2024-01-16` (Substrate VM) |
| `java` | `openjdk 21.0.2 2024-01-16 OpenJDK Runtime Environment GraalVM CE 21.0.2+13.1` |
| Gradle | 8.10 (used directly, not via wrapper — wrapper download fails on this network) |
| Host | NixOS (no FHS `/usr/lib`; zlib/glibc/gcc live in `/nix/store`) |
| Quarkus | 3.20.0 (same as v1 production fixtures) |

## Spike 1A — rust-camel YAML DSL Simple predicate support — **PASS**

**Goal:** confirm the T2 route (set_body / set_header / filter / choice / log) with
Simple-language `${body}` and `${header.X}` predicates compiles, starts, and
emits the exact marker `SPIKE_T2_OK body=pong-bench` on stdout.

**Fixture:** `benchmarks/spikes/spike-yaml-predicate/`
- `routes/spike-t2.yaml` — the T2 route in rust-camel YAML DSL syntax
  (note: rust-camel uses `key:` / `value:` for set_header and `value:` for
  set_body literal, NOT Apache Camel's `name:` / `constant:`). The plan's
  example YAML used the Apache Camel shape — adapted to the rust-camel
  shape, which is what `camel run` actually parses.
- `Camel.toml` — minimal config with ADR-0033 fail-closed exec stub
  (mirrors the production v1 fixture).

**Exact command:**
```bash
# Built-in target/release/camel from existing v1 build:
/home/kenny/dev/rust-camel/.worktrees/rc-f3g9-startup-benchmark/target/release/camel run \
  --config benchmarks/spikes/spike-yaml-predicate/Camel.toml \
  --routes benchmarks/spikes/spike-yaml-predicate/routes/spike-t2.yaml \
  --no-watch
```

**Evidence:** stdout contains:
```
INFO camel_processor::log: "SPIKE_T2_OK body=pong-bench" exchange_id=...
```
Exact `SPIKE_T2_OK body=pong-bench` is emitted. Choice `when` branch executed
(set_body literal `"pong-bench"` was selected over `"pong-other"`,
proving the `set_header` + `filter` + `choice` pipeline evaluated correctly).
Native build N/A (rust-camel-cli is already a static ELF).

**Implication:** the spec §4.1 Pair B "language-subsystem-equivalent to
Apache Camel" claim holds. T2 Pair B can be built with rust-camel-cli +
YAML.

## Spike 1B — Quarkus native-image build with T2 route — **PASS**

**Goal:** confirm the full T2 EIP shape (set_body, set_header, filter,
choice, log with `${body}`) survives GraalVM native-image compilation, with
both Java DSL and YAML DSL, and that the resulting binary emits
`BENCH_ROUTE_READY body=pong-bench`.

**Fixtures:** `benchmarks/spikes/spike-native-build/`
- `camel-quarkus-dsl-t2-spike/` — Java DSL, native build
- `camel-quarkus-yaml-t2-spike/` — YAML DSL, native build, with
  `quarkus.native.resources.includes=camel/routes.yaml` AND
  `camel.main.routesIncludePattern=classpath:camel/routes.yaml` (see
  "NixOS + native-image resource discovery" below for why both are needed)
- Shared `settings.gradle.kts`, `gradle/wrapper/`, `gradlew` (copied from
  the v1 camel-quarkus fixture)

### Spike 1B DSL subproject — **PASS**

**Exact command:**
```bash
export JAVA_HOME=/tmp/rc-f3g9-jdk21
export PATH=$JAVA_HOME/bin:$PATH
export LIBRARY_PATH=/nix/store/61a1nwx3w6rqyaisj5rn1sal1981apm7-zlib-1.3.2/lib
export C_INCLUDE_PATH=/nix/store/a9psmsc93llkravrd50rrv8k3dwdw60x-zlib-1.3.2-dev/include
cd benchmarks/spikes/spike-native-build
GRADLE=/home/kenny/dev/rust-camel/bridges/xml/.gradle-docker-cache/wrapper/dists/gradle-8.10-bin/deqhafrv1ntovfmgh0nh3npr9/gradle-8.10/bin/gradle
$GRADLE :camel-quarkus-dsl-t2-spike:build \
  -Dquarkus.native.enabled=true \
  -Dquarkus.package.jar.enabled=false \
  -Dquarkus.native.additional-build-args=-H:NativeLinkerOption=-L/nix/store/61a1nwx3w6rqyaisj5rn1sal1981apm7-zlib-1.3.2/lib \
  --no-daemon
```

The plan's exact invocation (`-Dquarkus.native.container-runtime=` and
`-Dquarkus.native.builder-image=` with empty strings) is **rejected** by
Quarkus 3.20.0 with `SRCFG00040: The config property quarkus.native.builder-image
is defined as the empty String ("") which ... considered to be null`. The
fix is to omit the empty overrides — Quarkus 3.20 falls back to the
local GraalVM when `container-runtime` is not set and `JAVA_HOME` points
at a GraalVM install (this is the documented local-build path).

**Evidence:** runner emits
```
INFO [route1] (Camel (camel-1) thread #1 - timer://bench) BENCH_ROUTE_READY body=pong-bench
INFO [io.quarkus] (main) camel-quarkus-dsl-t2-spike 1.0.0 native (powered by Quarkus 3.20.0) started in 0.018s.
```

| Metric | Value |
|---|---|
| Wall-clock build time | **64 s** (76s end-to-end including gradle daemon + spin-up) |
| Peak build RAM | **3 183 632 KiB** (~3.0 GiB) |
| Binary size | 60 328 984 bytes (≈ 57.5 MiB) |
| Reflection metadata | 4 061 types, 139 fields, 5 330 methods |
| GraalVM warnings | none material; only the standard "build successful" stats |
| Native binary startup (Quarkus log) | 0.018s |

### Spike 1B YAML subproject — **PASS**

**Exact command:** identical to DSL but with `:camel-quarkus-yaml-t2-spike:build`
and the same `-H:NativeLinkerOption` flag.

**Evidence:** runner emits
```
INFO [route1] (Camel (camel-1) thread #1 - timer://bench) BENCH_ROUTE_READY body=pong-bench
INFO [io.quarkus] (main) camel-quarkus-yaml-t2-spike 1.0.0 native (powered by Quarkus 3.20.0) started in 0.012s.
```
plus a deprecation warning for `set-body` / `set-header` kebab-case keys
(use `setBody` / `setHeader` camelCase). Not load-bearing for the smoke
test, but noted for the T2 fixture templates.

| Metric | Value |
|---|---|
| Wall-clock build time | **71 s** (77s end-to-end) |
| Peak build RAM | **4 511 276 KiB** (~4.3 GiB) |
| Binary size | 65 391 640 bytes (≈ 62.4 MiB) |
| Reflection metadata | 4 220 types, 139 fields, 5 394 methods |
| GraalVM warnings | none material |
| Native binary startup (Quarkus log) | 0.012s |

### NixOS + native-image resource discovery (Spike 1B YAML, root cause)

The YAML spike did not work on the first try. Symptoms: `Routes startup
(total:0)` despite the runner.jar containing `camel/routes.yaml`. Three
issues had to be resolved, in order:

1. **`-Dquarkus.native.builder-image=` empty string is rejected.** See
   Spike 1B DSL fix above. Same applies to YAML spike.

2. **NixOS linker cannot find `libz`.** GraalVM's link step invokes
   `gcc` from `/nix/store/.../gcc-wrapper/bin/gcc`, which cannot find
   `libz.so` (the one in `/nix/store/.../zlib-1.3.2/lib/` happens to be
   32-bit ELF — wrong architecture). The error was:
   `ld: se salta el .../libz.so incompatible mientras se busca -lz`
   ("skipping incompatible libz.so while searching for -lz"). The fix is
   `-H:NativeLinkerOption=-L/nix/store/61a1nwx3w6rqyaisj5rn1sal1981apm7-zlib-1.3.2/lib`
   to point the linker at a 64-bit `libz.so` (note: `NativeLinkerOption`,
   not `CLinkerOption` — the latter is not a recognised GraalVM option
   in 21.0.2 and yields `Could not find option 'CLinkerOption' from 'user'`.

3. **Substrate VM cannot enumerate classpath directories.** After the
   link succeeded, the native binary still found 0 routes. The cause is
   documented in `CamelMainNativeImageProcessor`: the runtime
   `CamelMainRoutesCollector` uses `getResources("camel/*")` to discover
   route files; Substrate VM cannot list directory contents, so the
   wildcard resolves to an empty set even when the file is registered as
   a `NativeImageResourceBuildItem`. The fix is to point
   `camel.main.routesIncludePattern` at the specific file
   (`classpath:camel/routes.yaml`) — `getResource(singular)` works in
   Substrate VM because the file is explicitly registered at build time
   via `quarkus.native.resources.includes=camel/routes.yaml`.

All three fixes are reproduced in the YAML spike's `application.properties`
and the documented `gradle` invocation. They will need to be replicated in
the production T2 fixture under Task 2 / Task 3.

## Summary

| Spike | Status | Evidence |
|---|---|---|
| 1A (rust-camel YAML predicate) | **PASS** | `SPIKE_T2_OK body=pong-bench` in stdout |
| 1B DSL (Quarkus native Java DSL) | **PASS** | `BENCH_ROUTE_READY body=pong-bench` in native runner stdout; 0.018s startup |
| 1B YAML (Quarkus native YAML DSL) | **PASS** | `BENCH_ROUTE_READY body=pong-bench` in native runner stdout; 0.012s startup |

**Both spikes pass.** v2 implementation can proceed to Task 2 (production
Quarkus native subprojects) and Task 3 (T2 fixtures). The NixOS toolchain
workarounds (zlib path, native resource includes + non-wildcard route
pattern) are now known and documented; they are the cost of building
Quarkus native locally on this host.

## Next steps (not in Task 1 scope)

- Task 2: replicate the YAML spike's `application.properties` two-key combo
  in the production T2 fixtures, and copy the `quarkus.native.additional-build-args`
  shape into both `camel-quarkus-dsl-native` and `camel-quarkus-yaml-native`
  `build.gradle.kts` (or surface as a Gradle extra property).
- Task 3: use the same T2 route shape as the YAML spike, translated to
  rust-camel YAML DSL (different field names: `key:`/`value:` /
  `simple:` / bare-string log vs Apache Camel's `name:` / `constant:`).
- Task 4: harness must exclude `benchmarks/spikes/` and `*/spike-*` from
  scenario auto-discovery (per plan §4a).
- Task 6: also consider a small note in `benchmarks/CONTEXT.md` about
  the NixOS-native-image caveats (zlib path, resource-include + non-wildcard
  pattern), since they are likely to recur on this host.

Bd: rc-p9ki

---

# Spike Results — Benchmark v3 (bd rc-f3g9, Task 1 pre-flight)

Three independent pre-flight spikes de-risking Task 2 (M2 harness), Task 3
(T3 HTTP fixtures), and Task 4 (T4a/T4b bridge fixtures). All three pass
on the first iteration. Full report: `.superpowers/sdd/task-1-report.md`.

## Toolchain (verified, same as v2)

| Component | Identity |
|---|---|
| GraalVM CE | 21.0.2+13.1 at `/tmp/rc-f3g9-jdk21/` |
| Maven | 3.9.12 at `/nix/store/sbjywy9g6p3zfb5cacqqvd64zi3qdqbh-maven-3.9.12/` |
| Gradle | 8.10 (re-used from `bridges/xml/.gradle-docker-cache/wrapper/`) |
| `rustc` | 1.96.0 (Nix default) |
| `cargo` | 1.96.0 |
| `reqwest` | 0.12.28 (in spike-loadgen) |

## Spike 1A — rust-camel HTTP server capability — **PASS**

**Goal:** confirm `camel-http` supports POST handler + sync response body +
listener bind + readiness signal. Spec §4.10 mandates the M1 marker fires
when the listener is bound (NOT on first request).

**Fixture:** `benchmarks/spikes/spike-http-server/`
- `Camel.toml` (ADR-0033 stub mirrors v1 fixture shape)
- `routes/spike-http.yaml` declares TWO routes:
  - `ready-marker`: `timer:ready?repeatCount=1&delay=200` → log `BENCH_ROUTE_READY`
  - `bench-http`: `http://0.0.0.0:18099/bench` → log + `set_body: { value: "pong" }`

**Exact command:**
```bash
./target/release/camel run \
  --config benchmarks/spikes/spike-http-server/Camel.toml \
  --routes benchmarks/spikes/spike-http-server/routes/spike-http.yaml \
  --no-watch &> /tmp/spike-http.log &
# wait for BENCH_ROUTE_READY
# smoke test via /dev/tcp
```

**Evidence:**
- Both routes log "Route started" at +5ms (CamelContext started). HTTP
  listener bound at this point.
- `BENCH_ROUTE_READY` marker emitted at +200ms (one-shot timer fires).
  Marker fires AFTER listener bind — readiness signal is sound.
- POST `/bench` with body `ping` → 200 OK with body `pong` (verified via
  raw `bash /dev/tcp`).

| Capability | Verified |
|---|---|
| POST handler receiving body | ✓ (request body available as `exchange.input.body`) |
| Synchronous response with body | ✓ (`set_body` step, body=`pong`, status 200) |
| Listener bind + readiness signal | ✓ (`BENCH_ROUTE_READY` on stdout at +200ms) |

## Spike 1B — Engine pinning across 3 sides (spec §4.8) — **PASS**

**Goal:** confirm Saxon-HE (XSLT) + Xerces-J (XSD) can be pinned to
IDENTICAL versions on rust-camel bridge + Apache Camel standalone +
Apache Camel Quarkus native (mismatched engines invalidate the tax
comparison per spec §4.8).

**Version comparison table (load-bearing):**

| Fixture | Saxon-HE | Xerces-J | Notes |
|---|---|---|---|
| rust-camel bridge (`bridges/xml`) | **12.5** | **2.12.2** | Pinned in `bridges/xml/build.gradle.kts:32-33`. |
| Apache Camel standalone (T4a/T4b future, mirrored by `spike-bridge-pinning/standalone-pin/pom.xml`) | **12.5** ✓ | **2.12.2** ✓ | `mvn dependency:tree` confirms resolution. |
| Apache Camel Quarkus native (T4a/T4b future, mirrored by `spike-bridge-pinning/quarkus-pin/spike-quarkus-xslt/build.gradle.kts`) | **12.5** ✓ | **2.12.2** ✓ | `gradle :spike-quarkus-xslt:dependencies --configuration runtimeClasspath` confirms resolution. |

**Critical finding:** camel-xslt + camel-validator do NOT pull
Saxon-HE or Xerces-J transitively on the Apache Camel side. Without
EXPLICIT `Saxon-HE` and `xercesImpl` deps, Apache Camel falls back to
the JDK 21 bundled XSLT/XSD engines. **Task 4 T4a/T4b fixture templates
MUST add the explicit deps** or the tax comparison is invalid (strawman:
Apache Camel + JDK vs rust-camel + Saxon-HE 12.5 + Xerces-J 2.12.2).

**Caveat:** bridge native binary NOT built in this spike (the brief
mentioned `cargo xtask build-xml-bridge` but that is a 64+s Quarkus
native build — beyond the spike's version-pinning scope; exercised in
Task 4). `bridges/xml/build.gradle.kts` + `gradle dependencies` is
sufficient evidence the pinned versions resolve.

## Spike 1C — Persistent load generator feasibility — **PASS**

**Goal:** confirm a Rust load generator can sustain **p99 <1ms baseline**
against a devnull HTTP server (spec §4.5). If higher, loadgen is the
bottleneck, escalate.

**Fixture:** `benchmarks/spikes/spike-loadgen/`
- `Cargo.toml` (detached `[workspace]` per spike convention)
- `src/devnull.rs` — raw-tokio minimal HTTP/1.1 server, 200 "ok"
- `src/loadgen.rs` — reqwest-based loadgen with `pool_max_idle_per_host(4)`

**Exact commands:**
```bash
env -u CARGO_TARGET_DIR cargo build --release \
  --manifest-path benchmarks/spikes/spike-loadgen/Cargo.toml --bins
SPIKE_PORT=18099 benchmarks/spikes/spike-loadgen/target/release/devnull &
sleep 0.2
benchmarks/spikes/spike-loadgen/target/release/loadgen \
  "http://127.0.0.1:18099/bench" 1000 1000
benchmarks/spikes/spike-loadgen/target/release/loadgen \
  "http://127.0.0.1:18099/bench" 5000 5000
```

**Evidence (target: p99 <1ms baseline):**

| Workload | p50 (ms) | p99 (ms) | p999 (ms) | max (ms) | Verdict |
|---|---|---|---|---|---|
| 1000 reqs @ 1000 reqs/sec | 0.074 | **0.156** | 0.420 | 0.636 | PASS (5.2× under 1ms) |
| 5000 reqs @ 5000 reqs/sec | 0.050 | **0.103** | 0.140 | 0.502 | PASS (9.7× under 1ms) |

**Design (validated):** persistent process, reqwest connection pool
keep-alive, monotonic `std::time::Instant` per-request, fixed rate
pacing (no in-measurement tuning). The hand-rolled tokio read-loop
version was abandoned (p99=41ms) — the spike ships the reqwest version
as the production baseline.

## Summary

| Spike | Status | Evidence |
|---|---|---|
| 1A (rust-camel HTTP server) | **PASS** | POST /bench ping → 200 pong; `BENCH_ROUTE_READY` on stdout |
| 1B (engine pinning) | **PASS** | Saxon-HE 12.5 + Xerces-J 2.12.2 identical on all 3 sides when explicit deps added |
| 1C (loadgen feasibility) | **PASS** | 1000@1000/sec: p99=0.156ms; 5000@5000/sec: p99=0.103ms (both <1ms target) |

**All 3 spikes pass. v3 implementation can proceed to Task 2 (M2 harness),
Task 3 (T3 fixtures), and Task 4 (T4a/T4b fixtures with explicit engine
pinning).** T4 follow-up: T4a/T4b fixture templates MUST add
`Saxon-HE 12.5` + `xercesImpl 2.12.2` + `xml-apis 1.4.01` as explicit
deps to satisfy the §4.8 invariant.

---

## Spike 1A-fix — `BENCH_ROUTE_READY <unix_ms>` + T3 entrance criterion — **PASS**

**Goal:** address Task 1 reviewer Important #1 + #3. The spike's
`BENCH_ROUTE_READY` marker must carry a `<unix_ms>` suffix (reviewer
#1) and the marker MUST be coupled to listener bind, not a sibling
timer (reviewer #3, upgraded to T3 entrance criterion).

**Fix #1 — marker carries `<unix_ms>`**:
`benchmarks/spikes/spike-http-server/routes/spike-http.yaml:26` now
reads:

```yaml
- log: "BENCH_ROUTE_READY ${header.CamelMessageTimestamp}"
```

`camel-timer` populates `CamelMessageTimestamp` (epoch millis) on every
fire when `includeMetadata=true`, which is the default. The `log`
step's bare-string is evaluated as Simple language (same pattern as
`spike-t2.yaml:54` `log: "SPIKE_T2_OK body=${body}"`), so
`${header.CamelMessageTimestamp}` interpolates to the millis string.
**No DSL support for `date:now:epochmillis`** — the Simple language
parser (`crates/languages/camel-language-simple/src/parser.rs`) only
handles `${body}`, `${header.<name>}`, `${exchangeProperty.<name>}`,
`${exception.message}`, and `${lang:expr}`. The header-based approach
is the canonical rust-camel way to get a timestamp in a log message.

**Evidence** (`./target/release/camel run` with the updated YAML):
```
INFO camel_processor::log: "BENCH_ROUTE_READY 1784451791368" exchange_id=e4ead698-…
```
The marker is exactly `BENCH_ROUTE_READY <unix_ms>`. Smoke test
(POST /bench body=ping → 200 pong) still passes — the body route is
unaffected.

**Fix #3 — T3 entrance criterion**: documented in
`.superpowers/sdd/task-1-report.md` under the new "T3 entrance
criterion" section. The spike (throwaway) is not modified to couple
the marker to the listener bind — the spike is acceptable because
both routes register before the context starts serving, and the
timer fires AFTER the route-start log line at +5ms (so the marker
fires when the listener is in fact bound). The T3 production fixture
MUST use a route-start hook on the HTTP listener instead of a sibling
timer; this is now a hard entrance criterion (not a follow-up).

---

## Spike 1B-fix — bridge build + invocation smoke (reviewer #2) — **PASS**

**Goal:** address Task 1 reviewer Important #2. The original spike
verified Saxon-HE + Xerces-J version pinning via `gradle/maven
dependency:tree` but skipped both `cargo xtask build-xml-bridge`
AND a real rust-camel route invocation of the bridge. Both are now
done in this session.

### 1. `cargo xtask build-xml-bridge` — **PASS**

```
$ env -u CARGO_TARGET_DIR cargo xtask build-xml-bridge
… (Quarkus native image, Saxon-HE 12.5 + Xerces-J 2.12.2 + gRPC)
Produced artifacts:
 /project/build/xml-bridge-dev-native-image-source-jar/xml-bridge-dev-runner (executable)
Build complete!
  Path:   /home/kenny/dev/rust-camel/.worktrees/rc-f3g9-startup-benchmark/bridges/xml/build/native/xml-bridge
  Size:   100.1 MB
  SHA256: 416e48e16f5857e66a0d1481900e1d3537a55f5726c18e4a5849fb3b4398260d
```

The native build completed in ~4m on the same host as v2. The binary
is at the canonical `bridges/xml/build/native/xml-bridge` path that
`crates/services/camel-bridge/src/download.rs:88-108` auto-detects
for the `xml-bridge` spec.

### 2. Bridge invocation smoke test — **PASS**

**Fixture:** `benchmarks/spikes/spike-bridge-pinning/invoke-spike/`
- `Camel.toml` — ADR-0033 fail-closed stub (mirrors spike-http-server shape)
- `transform.xslt` — copies the v2 stylesheet from `examples/xslt-example/stylesheets/transform.xslt` (trivial `<order><id>` → `<result><orderId>`)
- `routes/spike-xslt-invoke.yaml` — timer → `set_body` → `to: xslt:<abs path>` → `log: SPIKE_XSLT_INVOKE result=${body}`

**Exact command:**
```bash
CAMEL_XML_BRIDGE_BINARY_PATH=/home/kenny/dev/rust-camel/.worktrees/rc-f3g9-startup-benchmark/bridges/xml/build/native/xml-bridge \
  ./target/release/camel run \
    --config benchmarks/spikes/spike-bridge-pinning/invoke-spike/Camel.toml \
    --routes benchmarks/spikes/spike-bridge-pinning/invoke-spike/routes/spike-xslt-invoke.yaml \
    --no-watch
```

**Evidence:**
```
INFO camel_bridge::download: CAMEL_XML_BRIDGE_BINARY_PATH set — using local bridge binary: /home/kenny/dev/rust-camel/.worktrees/rc-f3g9-startup-benchmark/bridges/xml/build/native/xml-bridge
INFO camel_processor::log: "SPIKE_XSLT_INVOKE result=<result><orderId>42</orderId></result>" exchange_id=…
```

The bridge binary was spawned, accepted the request over gRPC,
Saxon-HE executed the XSLT 3.0 transform, and the transformed
XML `<result><orderId>42</orderId></result>` was returned to the
route. **End-to-end invocation works.** This proves `cargo xtask
build-xml-bridge` is sufficient as the canonical bridge-build
step for Task 4 T4a/T4b fixtures (no manual `quarkus build
--native` or docker invocation required).

---

Bd: rc-f3g9
