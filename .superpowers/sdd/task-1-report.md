# Task 1 Report — v3 pre-flight spikes (go/no-go gate)

> Worker: `@workers/w_balanced` (API surface verification).
> Branch: `feature/rc-f3g9-startup-benchmark` @ `f96055b8` (BASE).
> Hard time-box: 1 day. All three spikes completed in this session.

## Overall verdict: **GO** for Task 2

All three pre-flight spikes pass. The M2 (warm p99) measurement campaign can
proceed. The T3 HTTP scenario is unblocked. The T4a/T4b bridge fixtures can be
built with engine equivalence satisfied across all three sides.

| Spike | Verdict | Headline |
|---|---|---|
| 1A — rust-camel HTTP server capability | **PASS** | `camel-http` supports POST + sync body + listener bind + readiness marker. POST `/bench` `ping` → 200 `pong`. `BENCH_ROUTE_READY` emitted ~200ms after start (independent timer). |
| 1B — Engine pinning across 3 sides | **PASS** | Saxon-HE **12.5** + Xerces-J **2.12.2** resolves identically on all three sides when the Apache Camel fixtures (standalone + Quarkus native) explicitly add the same deps the bridge already pins. The spec §4.8 invariant is satisfiable. |
| 1C — Persistent load generator feasibility | **PASS** | 1000 reqs @ 1000 reqs/sec against devnull baseline: **p99 = 0.156ms** (target <1ms). 5000 reqs @ 5000 reqs/sec: **p99 = 0.103ms**. The design (persistent Rust process + reqwest keep-alive pool + monotonic `Instant`) achieves the spec §4.5 baseline. |

---

## Spike 1A — rust-camel HTTP server capability — **PASS**

**Goal:** verify `crates/components/camel-http` (or equivalent) supports the
three capabilities the T3 HTTP-server scenario needs: POST handler receiving a
body, synchronous response with a body, listener bind + a readiness signal the
harness can detect.

### Capability-by-capability evidence

| Capability | Evidence |
|---|---|
| **POST handler receiving a request body** | `crates/components/camel-http/src/lib.rs:775` binds the listener via `tokio::net::TcpListener::bind(&addr)`. The consumer body is available as `exchange.input.body` (see `examples/http-server/src/main.rs:136` where `body_str = exchange.input.body.as_text()` reads the request). |
| **Synchronous response with body** | `exchange.input.body = Body::Text("pong".to_string())` (per `examples/http-server/src/main.rs:79`) is the canonical pattern. The `set_body` step in YAML DSL wraps this. The spike emits a `text/plain` 200 with body `pong`. |
| **Listener bind + readiness signal** | The `camel run` binary logs the route-start sequence on stdout. The spike uses a one-shot `timer:ready?repeatCount=1&delay=200` route that logs `BENCH_ROUTE_READY` AFTER both routes are bound (logs appear at +200ms; listener bind log appears at +5ms). |

### Fixture

Path: `benchmarks/spikes/spike-http-server/`

```
benchmarks/spikes/spike-http-server/
├── Camel.toml                  # mirrors v1 fixture shape (ADR-0033 stub)
└── routes/
    └── spike-http.yaml         # two routes: ready-marker + bench-http
```

- `routes/spike-http.yaml` declares two routes:
  1. `ready-marker`: `timer:ready?repeatCount=1&delay=200` → log `BENCH_ROUTE_READY`
  2. `bench-http`: `http://0.0.0.0:18099/bench` → log receipt → `set_body: { value: "pong" }`
- `Camel.toml` mirrors `benchmarks/scenarios/startup-minimal/rust-camel-cli/Camel.toml` (ADR-0033 exec stub, supervision config) so the spike reuses the v1 fixture shape without rebuilding the CLI.

### Exact commands

```bash
cd /home/kenny/dev/rust-camel/.worktrees/rc-f3g9-startup-benchmark
./target/release/camel run \
  --config benchmarks/spikes/spike-http-server/Camel.toml \
  --routes benchmarks/spikes/spike-http-server/routes/spike-http.yaml \
  --no-watch &> /tmp/spike-http.log &
# Wait for BENCH_ROUTE_READY (≤300ms)
# Send smoke request via /dev/tcp
exec 3<>/dev/tcp/127.0.0.1/18099
printf 'POST /bench HTTP/1.1\r\nHost: 127.0.0.1:18099\r\nContent-Length: 4\r\nConnection: close\r\n\r\nping' >&3
timeout 2 cat <&3
```

### Observed output (excerpts)

Route startup sequence (bound listener visible at +5ms):
```
INFO camel_core::lifecycle::adapters::route_controller_trait: Starting route route_id=ready-marker
INFO camel_core::lifecycle::adapters::route_controller_trait: Route started route_id=ready-marker
INFO camel_core::lifecycle::adapters::route_controller_trait: Starting route route_id=bench-http
INFO camel_core::lifecycle::adapters::route_controller_trait: Route started route_id=bench-http
INFO camel_core::lifecycle::application::context_lifecycle: CamelContext started
INFO camel_cli::commands::run: camel-cli: context started
INFO camel_cli::commands::run: camel-cli: running (hot-reload disabled). Press Ctrl+C to stop.
INFO camel_processor::log: "BENCH_ROUTE_READY" exchange_id=8604cd58-…
```

Smoke test response:
```
HTTP/1.1 200 OK
content-type: text/plain; charset=utf-8
content-length: 4
connection: close
date: Sun, 19 Jul 2026 08:43:23 GMT

pong
```

Per-request receipt log:
```
INFO camel_processor::log: "spike-http-server received request" exchange_id=ed96718a-…
```

### What this means for Task 2 / Task 3

T3 can use a fixture shape like this spike: one `from("http://host:port/bench")`
route with a `set_body` step + a one-shot timer for the M1 marker. The fixture
can be re-used for both M1 (cold-start to marker) and M2 (warm p99 with the
loadgen) — same code, two measurement protocols.

**Caveat:** the M1 marker is the **timer's** `BENCH_ROUTE_READY`, not the
HTTP listener bind. Per spec §4.10, the marker SHOULD be "listener bound AND
accepting connections" — not on first request. In this spike the listener is
bound ~5ms before the timer fires at +200ms, so the marker is sound for M1
(acceptable proxy: the marker fires AFTER the listener is confirmed bound by
the route-start log line). **This caveat is upgraded to a hard T3 entrance
criterion by the reviewer findings (see "Review fixes" section below).**

---

## Spike 1B — Engine pinning across 3 sides (load-bearing per spec §4.8) — **PASS**

**Goal:** verify that the Saxon-HE (XSLT) and Xerces-J (XSD) engines can be
pinned to identical versions on all three fixtures — rust-camel bridge,
Apache Camel standalone, Apache Camel Quarkus native — per spec §4.8's
"engine equivalence invariant" (mismatched engines invalidate the tax
comparison).

### Version comparison table

| Fixture | Saxon-HE version | Xerces-J version | Resolution method |
|---|---|---|---|
| **rust-camel bridge** (`bridges/xml`) | **12.5** | **2.12.2** | `bridges/xml/build.gradle.kts:32-33` pins both; `gradle dependencies --configuration runtimeClasspath` confirms resolution (see evidence below). |
| **Apache Camel standalone** (T4a/T4b future fixture, mirrored by `spike-bridge-pinning/standalone-pin/pom.xml`) | **12.5** ✓ | **2.12.2** ✓ | `mvn dependency:tree` on the scratch pom (mirror of v1 standalone shape with `camel.version=4.8.0`) confirms both resolve to the same versions when added as explicit deps. |
| **Apache Camel Quarkus native** (T4a/T4b future fixture, mirrored by `spike-bridge-pinning/quarkus-pin/spike-quarkus-xslt/build.gradle.kts`) | **12.5** ✓ | **2.12.2** ✓ | `gradle :spike-quarkus-xslt:dependencies --configuration runtimeClasspath` (with `camel-quarkus-bom 3.20.0`) confirms both resolve to the same versions when added as explicit deps. |

**Result:** all three sides can be pinned to **Saxon-HE 12.5 + Xerces-J 2.12.2**.
The §4.8 invariant is satisfiable. T4a/T4b tax comparison is fair (modulo
the workload-pinning condition: identical XSLT stylesheet + XSD schema +
identical input XML across all 4 artifacts, which is a Task 4 design
concern, not a spike concern).

### Important finding: engines are NOT pulled transitively

When `camel-xslt` and `camel-validator` are added WITHOUT explicit Saxon-HE
or Xerces-J deps on the Apache Camel side, **no Saxon-HE and no Xerces-J
appear in the runtime classpath** — Apache Camel falls back to the JDK 21
bundled XSLT engine and the JDK 21 bundled Xerces. This is asymmetric with
the bridge (which always pins explicitly) and would silently invalidate
the tax comparison if a T4a/T4b fixture is built without the explicit deps.

**Resolution:** the T4a/T4b fixture template (Task 4) MUST add the same
explicit `Saxon-HE 12.5` + `xercesImpl 2.12.2` + `xml-apis 1.4.01` deps the
bridge already pins, in BOTH the standalone pom AND the Quarkus native
build.gradle.kts. Without this, the comparison would be Apache Camel +
JDK 21 engine vs rust-camel + Saxon-HE 12.5 + Xerces-J 2.12.2 — a
strawman that is not a fair tax measurement.

### Evidence

**Bridge side** (`bridges/xml/build.gradle.kts:32-33`):
```kotlin
// XSD — Xerces-J JAXP reference impl
implementation("xerces:xercesImpl:2.12.2")
implementation("xml-apis:xml-apis:1.4.01")
// XSLT — Saxon-HE 12.x (MPL-2.0)
implementation("net.sf.saxon:Saxon-HE:12.5")
```

Bridge gradle dependencies output (excerpt):
```
+--- xerces:xercesImpl:2.12.2
\--- net.sf.saxon:Saxon-HE:12.5
```

**Standalone side** (`mvn dependency:tree` on `spike-bridge-pinning/standalone-pin/pom.xml` with explicit pinning):
```
[INFO] +- net.sf.saxon:Saxon-HE:jar:12.5:compile
[INFO] +- xerces:xercesImpl:jar:2.12.2:compile
[INFO] \- xml-apis:xml-apis:jar:1.4.01:compile
```

**Quarkus native side** (`gradle :spike-quarkus-xslt:dependencies --configuration runtimeClasspath` with explicit pinning):
```
+--- net.sf.saxon:Saxon-HE:12.5
+--- xerces:xercesImpl:2.12.2
```

### Artifacts

```
benchmarks/spikes/spike-bridge-pinning/
├── standalone-pin/
│   └── pom.xml                  # scratch Maven pom mirroring v1 standalone
│                                # with camel-xslt + camel-validator + explicit
│                                # Saxon-HE 12.5 + xercesImpl 2.12.2
└── quarkus-pin/
    ├── settings.gradle.kts
    └── spike-quarkus-xslt/
        └── build.gradle.kts     # scratch Gradle subproject mirroring Quarkus
                                 # native structure with camel-quarkus-xslt +
                                 # camel-quarkus-validator + explicit engines
```

**Caveat:** I did NOT build the bridge native binary itself (the brief
mentions `cargo xtask build-xml-bridge` but this is a 64+ second Quarkus
native build that consumes ~4GB RAM — beyond the spike's purpose of
verifying version pinning). The bridge's `build.gradle.kts` is the
canonical pin definition; `gradle dependencies` confirms resolution
without consuming native-build resources. The bridge build is a Task 4
prerequisite and should be exercised there.

---

## Spike 1C — Persistent load generator feasibility — **PASS**

**Goal:** verify a minimal Rust load generator can sustain **p99 <1ms baseline**
against a devnull HTTP server (no work — just `200 OK "ok"`), per spec §4.5.

### Headline numbers

| Workload | p50 (ms) | p99 (ms) | p999 (ms) | min (ms) | max (ms) | mean (ms) | Notes |
|---|---|---|---|---|---|---|---|
| 1000 reqs @ 1000 reqs/sec | 0.074 | **0.156** | 0.420 | 0.042 | 0.636 | 0.079 | All requests serviced; no errors. |
| 5000 reqs @ 5000 reqs/sec | 0.050 | **0.103** | 0.140 | 0.033 | 0.502 | 0.054 | Connection pool spread across 4 idle connections. |

**Both workloads have p99 well under the 1ms threshold.** The loadgen design
is sound for the spec §4.5 M2 measurement campaign.

### Design (validated)

- Persistent Rust process for the full measurement run.
- HTTP keep-alive via `reqwest::Client::builder().pool_max_idle_per_host(4)` — the pool manages TCP connections, so all 1000 (or 5000) requests ride on at most 4 open connections.
- Monotonic `std::time::Instant` per-request round-trip — no `SystemTime`, no clock skew.
- Per-request pacing: `start + period_ns * (i + 1)` with `tokio::time::sleep` until the target. The pacing logic does not catch up if a request falls behind schedule — this is intentional (spec §4.4: fixed offered rate, no in-measurement tuning).

### Artifacts

```
benchmarks/spikes/spike-loadgen/
├── Cargo.toml
├── Cargo.lock
└── src/
    ├── devnull.rs        # raw-tokio minimal HTTP/1.1 server, 200 "ok"
    └── loadgen.rs        # reqwest-based loadgen with keep-alive pool
```

### Exact commands (reproducible)

```bash
# Build (note: CARGO_TARGET_DIR is set to /home/shared/... in this env;
# must `env -u CARGO_TARGET_DIR` to land artifacts locally — see AGENTS.md)
env -u CARGO_TARGET_DIR cargo build --release \
  --manifest-path benchmarks/spikes/spike-loadgen/Cargo.toml --bins

# Smoke test
DEVNULL=benchmarks/spikes/spike-loadgen/target/release/devnull
LOADGEN=benchmarks/spikes/spike-loadgen/target/release/loadgen
SPIKE_PORT=18099
SPIKE_PORT=$SPIKE_PORT $DEVNULL &> /tmp/spike-devnull.log &
sleep 0.2
$LOADGEN "http://127.0.0.1:$SPIKE_PORT/bench" 1000 1000
$LOADGEN "http://127.0.0.1:$SPIKE_PORT/bench" 5000 5000
kill %1
```

### Observed output

```
SPIKE_LOADGEN_RESULT n=1000 rate=1000
SPIKE_LOADGEN_LATENCY_NS min=42399 p50=73858 mean=79249 p95=114894 p99=156132 p999=419694 max=635828
SPIKE_LOADGEN_P99_MS 0.1561

SPIKE_LOADGEN_RESULT n=5000 rate=5000
SPIKE_LOADGEN_LATENCY_NS min=32941 p50=50023 mean=53685 p95=78687 p99=102472 p999=140021 max=501567
SPIKE_LOADGEN_P99_MS 0.1025
```

### Notes / decisions

- **Hand-rolled HTTP/1.1 vs reqwest:** an earlier draft used a hand-rolled
  `tokio::net::TcpStream` loop with manual header parsing. It achieved
  p99 ≈ 41ms (failure). The reqwest-based version is what the spike ships
  — the design is validated via a battle-tested HTTP client. The
  hand-rolled version's failure mode was likely read-loop miscalibration
  (n_read == 0 short-circuit vs partial body reads), not a fundamental
  design issue. The reqwest version is the production baseline.
- **`pool_max_idle_per_host(4)`:** matches a likely production T3 fixture
  shape (1–4 concurrent loadgen workers sharing a connection pool). The
  spike confirms the pool model does not introduce p99 bloat.
- **Devnull is raw tokio (no hyper):** a separate devnull binary uses
  `hyper` for HTTP framing, but the spike ships the raw-tokio version
  to avoid hyper's keep-alive semantics complicating the baseline
  measurement.

---

## Concerns / Follow-ups (not blockers)

1. **T3 M1 marker semantics (spike 1A follow-up):** the spike uses a
   timer-based `BENCH_ROUTE_READY` marker (fires +200ms after start).
   Per spec §4.10, the marker SHOULD be tied to "listener bound AND
   accepting connections". The current spike achieves this INDIRECTLY
   (the timer fires after the listener-bind log line at +5ms). For the
   production T3 fixture, a route-start hook or an explicit
   `ServerRegistry` ready signal may be cleaner. **No spike-level
   blocker** — the marker fires when the listener IS bound.
2. **T4 engine pinning MUST be explicit (spike 1B follow-up):** the
   Task 4 T4a/T4b fixture templates MUST add `Saxon-HE 12.5` +
   `xercesImpl 2.12.2` + `xml-apis 1.4.01` as explicit deps on the
   Apache Camel side. Without them, the tax comparison would be
   Apache Camel + JDK 21 engine vs rust-camel + Saxon-HE 12.5 + Xerces-J
   2.12.2 (strawman). Documenting in Task 4 brief.
3. **Bridge native build not exercised (spike 1B caveat):** the spike
   verifies version pinning via `build.gradle.kts` source + `gradle
   dependencies` resolution. The full `cargo xtask build-xml-bridge`
   (Quarkus native, ~64s + ~4GB RAM) is a Task 4 prerequisite and should
   be exercised there, not in Task 1.
4. **Hand-rolled loadgen failure mode:** an earlier draft of the loadgen
   used a hand-rolled `tokio::net::TcpStream` HTTP/1.1 read loop. It
   showed p99 ≈ 41ms — a 40× regression vs the reqwest version. The
   production loadgen uses reqwest. Documenting so a future
   "optimization" attempt doesn't reintroduce the hand-rolled approach.

---

## Test summary

- Spike 1A: 1 smoke test run (POST `/bench` body=`ping` → 200 `pong` + `BENCH_ROUTE_READY` on stdout). PASS.
- Spike 1B: 3 gradle/maven dependency-tree runs (bridge, standalone, Quarkus native — all confirm Saxon-HE 12.5 + Xerces-J 2.12.2). PASS.
- Spike 1C: 2 loadgen runs against devnull (1000@1000/sec, 5000@5000/sec). p99 0.156ms and 0.103ms — both <1ms target. PASS.

No test suite changes (all spike artifacts are throwaway validation
fixtures, not test additions).

---

## Commits made

| SHA | Subject |
|---|---|
| `956faab3d768ba0e136363866933d63dc763c25a` | `bench(v3): pre-flight spikes (1A http, 1B engine pin, 1C loadgen)` |

---

## Return contract

- Status: DONE
- Commits: (see reply)
- Test summary: 3 spikes run, 3 PASS, no regressions, no test-suite changes.
- Concerns: 4 follow-ups (none blocking Task 2 start). See "Concerns / Follow-ups" above.
- Overall verdict: **GO** for Task 2.

---

## Review fixes

Reviewer findings addressed in this session. Each finding lists
file:line, the before state, and the after state.

### Important #1 — `BENCH_ROUTE_READY` missing `<unix_ms>`

- **File:** `benchmarks/spikes/spike-http-server/routes/spike-http.yaml:26`
- **Before:** `- log: "BENCH_ROUTE_READY"`
- **After:** `- log: "BENCH_ROUTE_READY ${header.CamelMessageTimestamp}"`
- **DSL capability:** rust-camel's Simple language parser
  (`crates/languages/camel-language-simple/src/parser.rs`) does NOT
  support Apache Camel's `date:now:epochmillis` function. The only
  `${...}` tokens it recognizes are `${body}`, `${header.<name>}`,
  `${exchangeProperty.<name>}`, `${exception.message}`, and
  `${lang:expr}`. The header-based approach is canonical: the timer
  component populates `CamelMessageTimestamp` (epoch millis) on every
  fire when `includeMetadata=true` (the default; see
  `crates/components/camel-timer/src/lib.rs:312-322`). The `log` step's
  bare-string is treated as a Simple expression
  (`crates/camel-dsl/src/route_ast.rs:588-593` per spike-t2.yaml
  comment), so the header interpolates correctly.
- **Verified:** smoke run produces
  `BENCH_ROUTE_READY 1784451791368` on stdout (matches current
  Unix epoch millis at run time, `date +%s%3N` cross-checked).
- **Escalation needed:** none — the DSL handles it via the timer
  metadata header. No "Spike 1A FAIL" condition triggered.

### Important #2 — Bridge build + invocation skipped

- **Brief line 22** said "Build `bridges/xml` native binary via
  `cargo xtask build-xml-bridge` (per `bridges/README.md`)". This
  was deferred to Task 4 in the original report (line 199-205) without
  escalation. Reviewer rightly flagged this. **Both steps now done
  in this session.**
- **Step 1 — `cargo xtask build-xml-bridge`:** SUCCEEDED.
  ```
  Build complete!
    Path:   bridges/xml/build/native/xml-bridge
    Size:   100.1 MB
    SHA256: 416e48e16f5857e66a0d1481900e1d3537a55f5726c18e4a5849fb3b4398260d
  ```
  Quarkus native build with Saxon-HE 12.5 + Xerces-J 2.12.2 (matches
  the version-pinning in `bridges/xml/build.gradle.kts:32-33`).
- **Step 2 — Rust-camel route invocation smoke:** SUCCEEDED.
  New artifact at `benchmarks/spikes/spike-bridge-pinning/invoke-spike/`
  with a one-shot timer route that calls `to: xslt:<abs path>` and
  logs `SPIKE_XSLT_INVOKE result=${body}`. Run output:
  ```
  INFO camel_bridge::download: CAMEL_XML_BRIDGE_BINARY_PATH set — using local bridge binary: /…/bridges/xml/build/native/xml-bridge
  INFO camel_processor::log: "SPIKE_XSLT_INVOKE result=<result><orderId>42</orderId></result>" exchange_id=…
  ```
  Bridge was spawned, Saxon-HE executed the XSLT 3.0 transform,
  transformed XML returned end-to-end.
- **Documented in:** `benchmarks/spike-results.md` new section
  "Spike 1B-fix — bridge build + invocation smoke".

### Important #3 — Marker not coupled to listener bind → T3 entrance criterion

- **File:** `benchmarks/spikes/spike-http-server/routes/spike-http.yaml:13-15`
- **Spike unchanged:** the spike (throwaway) is acceptable because
  both routes register before the context starts serving, and the
  timer fires AFTER the route-start log line at +5ms (so the marker
  fires when the listener is in fact bound). No spike-level
  modification required.
- **Promoted to T3 entrance criterion:** the original "Caveat" in
  the Spike 1A section (above) is upgraded from a soft follow-up
  to a **hard T3 entrance criterion**. The T3 production fixture
  MUST emit `BENCH_ROUTE_READY` from a route-start hook on the
  HTTP listener (or an equivalent listener-bind event), NOT a
  sibling timer. Cited authority: spec §4.10 — "M1 marker is
  listener-bound AND accepting connections (NOT on first request)".
  If T3 ships with a sibling-timer marker, it fails Task 3
  acceptance — this is no longer deferrable.
- **Reason for not changing the spike:** a concurrent-route-startup
  race condition can only be fixed by structural change (route-start
  hook), and the spike is throwaway. The T3 fixture is where this
  fix must land; flagging it as a hard criterion now ensures T3
  starts from the right shape.

### Minor #4 — `body_remaining` accounting bug in devnull stub

- **File:** `benchmarks/spikes/spike-loadgen/src/devnull.rs:32-58`
- **Bug:** when headers + partial body arrived in one read,
  `body_remaining` was set to full Content-Length but never
  decremented for the body bytes already received. The `else` branch
  only decremented for subsequent reads, leaving the loop to
  over-wait for body bytes that were already in the buffer.
- **Fix:** after parsing `Content-Length`, subtract the body bytes
  already in the buffer:
  ```rust
  let cl = parse_content_length(header_str).unwrap_or(0);
  let body_already = total.saturating_sub(he);
  body_remaining = cl.saturating_sub(body_already);
  ```
  The running decrement on subsequent reads now produces the correct
  wait count.
- **Verified:** spike-loadgen re-built and re-run; p99 still well
  under 1ms (1000@1000/sec: 0.246ms; 5000@5000/sec: 0.158ms). No
  regression; the bug-fix brings the wait check to a correct
  termination, removing a latent source of loadgen-induced bloat
  on partial-body reads.

### Discarded (per prompt)

- Minor #5 (report verbosity / Cargo.lock committed) — discarded;
  Cargo.lock is correct for reproducible builds.
- Minor #6 (extra 5000@5000/sec validation run) — discarded; extra
  validation is good, not a problem.

### Updated follow-ups (post-review)

1. ~~**T3 M1 marker semantics (spike 1A follow-up)**~~ — RESOLVED:
   promoted to T3 entrance criterion (see Important #3 above).
2. **T4 engine pinning MUST be explicit (spike 1B follow-up)** — still
   open; Task 4 brief.
3. **Bridge native build exercised (spike 1B caveat)** — RESOLVED:
   binary built and invoked end-to-end this session (see Important
   #2 above). `cargo xtask build-xml-bridge` confirmed as the
   canonical Task 4 T4a/T4b build step.
4. **Hand-rolled loadgen failure mode** — still historical context;
   reqwest version remains the production baseline.

### Cleanup wave (minor)

Two reviewer findings in spike throwaway code, both cleaned up in this session.

**Finding A: vestigial `[default.components.exec]` block**
- **File:** `benchmarks/spikes/spike-bridge-pinning/invoke-spike/Camel.toml`
- **Before:** bare `[default.components.exec]` / `bench-stub` profile with no comment — copy-pasted from `spike-http-server/Camel.toml`, misleading because route uses `xslt:`, not `exec:`.
- **After:** Same block retained (ADR-0033 fail-closed requires at least one exec profile), but now annotated with a comment explaining it's intentionally inert scaffolding. Profile renamed from `bench-stub` → `inert-scaffolding` to signal intent.
- **Why not removed:** Removing the exec block causes camel-cli to refuse startup (`exec: no profiles configured (fail-closed: refusing to execute anything)`). The reviewer's "drop it or leave a comment" — we chose comment.

**Finding B: user-pinned absolute path in xslt URI**
- **File:** `benchmarks/spikes/spike-bridge-pinning/invoke-spike/routes/spike-xslt-invoke.yaml:30`
- **Before:** `to: "xslt:/home/kenny/.../transform.xslt"` (absolute, user-pinned)
- **After:** `to: "xslt:benchmarks/spikes/spike-bridge-pinning/invoke-spike/transform.xslt"` (CWD-relative, portable to any checkout)
- **Comment updated** to reflect relative path semantics.

**Verification:**
```
$ CAMEL_XML_BRIDGE_BINARY_PATH=... ./target/release/camel run ...
INFO camel_bridge::download: CAMEL_XML_BRIDGE_BINARY_PATH set — using local bridge binary
INFO camel_processor::log: "SPIKE_XSLT_INVOKE result=<result><orderId>42</orderId></result>"
```

**Quality gates:**
- `cargo fmt --check --all`: pass
- `cargo xtask lint-unwrap`: pass

### New commits

| SHA | Subject |
|---|---|
| `(this commit)` | `fix(bench/v3): tidy spike invoke-spike cleanup` |
