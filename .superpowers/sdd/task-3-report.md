# Task 3 Report — T3 HTTP Server Fixtures

bd: `rc-2vxg` · branch: `feature/rc-f3g9-startup-benchmark`

## 1. Mandated artifacts (4)

Per `.superpowers/sdd/task-3-brief.md`. Route shape (logical, identical across all 4):

```
from(jetty:http://0.0.0.0:8080/bench?httpMethodRestrict=POST)
  .log("BENCH_HTTP_REQUEST received")
  .setBody(constant("pong"))     # → 200 text/plain
```

### 1.1 camel-standalone-dsl (Pair A — Apache Camel 4.8.0 standalone JVM)

- **Files** (commit `df28a705`):
  - `benchmarks/scenarios/http-server/camel-standalone/pom.xml` (parent aggregator)
  - `benchmarks/scenarios/http-server/camel-standalone/camel-standalone-dsl/pom.xml`
  - `benchmarks/scenarios/http-server/camel-standalone/camel-standalone-dsl/src/main/java/com/rustcamel/bench/App.java`
- **Route (1-line)**: hardcoded Java DSL `from("jetty:http://0.0.0.0:8080/bench?httpMethodRestrict=POST").log(...).setBody(constant("pong"))`, `jetty:` is the server-side component in Camel 4.x.
- **Marker mechanism**: `App.java` installs an `EventNotifierSupport` listener filtered to `CamelEvent.Type.RouteStarted` (verifier `isEnabled` returns true only for that type) on the `CamelContext`'s management strategy **before** `context.start()`. `RouteStarted` fires only after the `camel-jetty` consumer's `start()` has returned from `JettyServer.start()`, which is the listener-bound + accept-loop-running instant. One-shot `AtomicBoolean MARKER_EMITTED` guards re-emit on supervision restarts. **Listener-bound ✓** (not sibling-timer, not first-request).
- **Smoke result**: marker emitted `BENCH_ROUTE_READY 1784464785352` after `Started ServerConnector@…{HTTP/1.1}{0.0.0.0:8080}`; per-request `BENCH_HTTP_REQUEST received` line present → POST succeeded.

### 1.2 camel-quarkus-dsl-native (Pair A — camel-quarkus 3.20.0 / Camel 4.10.2 native)

- **Files**:
  - `benchmarks/scenarios/http-server/camel-quarkus/settings.gradle.kts`
  - `benchmarks/scenarios/http-server/camel-quarkus/camel-quarkus-dsl/build.gradle.kts`
  - `benchmarks/scenarios/http-server/camel-quarkus/camel-quarkus-dsl/src/main/java/com/rustcamel/bench/BenchRoute.java`
  - `benchmarks/scenarios/http-server/camel-quarkus/camel-quarkus-dsl/src/main/resources/application.properties`
  - `benchmarks/scenarios/http-server/camel-quarkus/camel-quarkus-dsl-native/build.gradle.kts` (native sibling, shares JVM source via sourceSets)
- **Route (1-line)**: same `from("jetty:http://0.0.0.0:8080/bench?httpMethodRestrict=POST").log(...).setBody(constant("pong"))` in a CDI `@ApplicationScoped RouteBuilder`. `camel-jetty:4.8.0` pulled directly because `camel-quarkus-jetty` isn't published in 3.20 (documented workaround in `build.gradle.kts`).
- **Marker mechanism**: `BenchRoute.configure()` registers an `EventNotifierSupport` on the injected `CamelContext` during CDI init (before `context.start()`); notifier fires on `RouteStarted` event, emits `BENCH_ROUTE_READY <unix_ms>` once (`AtomicBoolean MARKER_EMITTED`). Same listener-bind argument as 1.1. **Listener-bound ✓**.
- **Smoke result**: marker emitted `BENCH_ROUTE_READY 1784464788359`; `camel-quarkus-dsl-native 1.0.0 native started in 0.011s`; per-request `BENCH_HTTP_REQUEST received` present → POST succeeded.

### 1.3 rust-camel-lib (Pair A — programmatic rust-camel)

- **Files**:
  - `benchmarks/scenarios/http-server/rust-camel-lib/Cargo.toml` (workspace member, path-deps into worktree crates)
  - `benchmarks/scenarios/http-server/rust-camel-lib/.cargo/config.toml` (fixture-local `target-dir`)
  - `benchmarks/scenarios/http-server/rust-camel-lib/src/main.rs`
- **Route (1-line)**: `RouteBuilder::from("http://0.0.0.0:8080/bench").route_id("bench-http").log("BENCH_HTTP_REQUEST received", Info).process(counter++).set_body("pong").build()`.
- **Marker mechanism**: `main.rs:146` spawns `emit_listener_bound_marker(host="127.0.0.1", port=8080)` after `ctx.start().await?`. The probe task (`main.rs:167-195`) performs `tokio::net::TcpStream::connect("127.0.0.1:8080")` in a loop with 1 ms back-off and a 30 s deadline; the first successful connect prints `BENCH_ROUTE_READY <unix_ms>`. The TCP connect can only succeed against an accept()-ing listener. **Listener-bound ✓** (TCP-probe, not sibling-timer). Caveat: probe succeeds against *any* process bound on 8080, not necessarily this process — see Concerns §4.
- **Smoke result**: marker emitted `BENCH_ROUTE_READY 1784464772274`, but the same log shows `Consumer error: Endpoint creation failed: Failed to bind 0.0.0.0:8080: Address already in use (os error 98)` immediately before the marker. The probe succeeded against an orphan listener left over from a prior smoke run (stale port), not this process's listener. No `BENCH_HTTP_REQUEST` line recorded → POST contract NOT verified for this smoke run. **Re-run needed with verified-clean port** before claiming smoke PASS.

### 1.4 rust-camel-cli (Pair B — `camel run` + YAML DSL + wrapper)

- **Files**:
  - `benchmarks/scenarios/http-server/rust-camel-cli/Camel.toml` (ADR-0033 fail-closed stub, bare-`true` exec profile)
  - `benchmarks/scenarios/http-server/rust-camel-cli/routes/http-server.yaml` (`from: http://0.0.0.0:8080/bench` + `log: BENCH_HTTP_REQUEST received` + `set_body: { value: "pong" }`)
  - `benchmarks/scenarios/http-server/rust-camel-cli/http-server-cli-wrapper.sh`
- **Route (1-line)**: `from: http://0.0.0.0:8080/bench` → `log: BENCH_HTTP_REQUEST received` → `set_body: { value: "pong" }`. Loaded by `camel run --config Camel.toml --routes routes/http-server.yaml`.
- **Marker mechanism**: rust-camel's YAML DSL has no route-start hook, so the wrapper script (`http-server-cli-wrapper.sh`) spawns `camel run` via `setsid`, then probes with `(exec 3<>"/dev/tcp/127.0.0.1/8080")`; on first successful open, emits `BENCH_ROUTE_READY <unix_ms>`. **Listener-bound ✓** (TCP-probe in wrapper). Same orphan-port caveat as 1.3.
- **Smoke result**: marker emitted `BENCH_ROUTE_READY 1784464784752` *before* the `camel run` child had finished starting its CamelContext (child's "Starting CamelContext" log line appears *after* the wrapper's marker) — same symptom as 1.3, probe hit a stale orphan listener. The child's own consumer later logged `Failed to bind 0.0.0.0:8080: Address already in use`. No `BENCH_HTTP_REQUEST` recorded. **Re-run needed with verified-clean port**.

## 2. Extras decision

**Kept.** Pattern matches v2 (`startup-minimal/`, `t2-realistic-eip/`).

Both v2 scenarios ship the full Pair A/B × {JVM, native} matrix per Camel-Quarkus contender (4 subprojects: `camel-quarkus-dsl`, `camel-quarkus-dsl-native`, `camel-quarkus-yaml`, `camel-quarkus-yaml-native`) plus `camel-standalone-dsl` + `camel-standalone-yaml` for the standalone contender. T3 ships the identical matrix. Removing the YAML/native extras would diverge from v2's structure and break the per-pair fairness model documented in `benchmarks/CONTEXT.md` §3.

The brief lists 4 top-level fixture dirs but does not preclude the matrix expansion each v2 fixture dir already contains; the worker followed the v2 structural template.

## 3. Commits

| SHA | Subject |
|---|---|
| `df28a705` | `feat(bench/v3): add T3 http-server fixtures` (27 files, 1725 LOC) |
| `bbefda92` | `chore(bench/v3): register T3 in workspace + harness` (3 files) |
| `0368bafd` | `test(bench/v3): add T3 smoke logs` (9 files) |

All carry `Bd: rc-2vxg` trailer per `AGENTS.md` caveman-commit convention.

## 4. Concerns for Task 5 review

- **Orphan-port false-positive on TCP-probe markers (rust-camel-lib + rust-camel-cli).** Both Pair A and Pair B rust-camel markers are TCP-probe based (connect to 127.0.0.1:8080, emit on first success). The probe is structurally listener-bound — it cannot succeed against a port no one is bound on — but it cannot distinguish *this* process's listener from a stale orphan from a prior cell. The smoke logs for 1.3 and 1.4 demonstrate the failure mode: marker fired against an orphan, child failed with `EADDRINUSE`, POST contract silently violated. **Task 5 (M1 harness) MUST** either (a) `pkill -9` by PID file before each cell with verified port release (current smoke `free_port_8080` is insufficient — see smoke log timestamps), or (b) seed camel-http with a listener-bound hook so the marker emits from inside the runtime, decoupled from any external process holding 8080.
- **Smoke not a clean re-run for rust-camel-lib / rust-camel-cli.** The committed smoke logs preserve the orphan-affected evidence (intentionally — they're the historical record). Before Task 5 reports numbers for T3 Pair A/B rust-camel, smoke MUST be re-run after `pkill -9 -f 'camel|http-server' && ss -tlnp | grep :8080` returns empty.
- **`benchmarks/harness/run.sh` registers T3 in `SCENARIO_MARKER` only.** No artifact-resolution case for `http-server` was added to `resolve_scenario_artifacts` — the harness cannot yet launch T3 cells. This is consistent with the task brief (Task 5 owns harness integration) but reviewers should not assume T3 is harness-ready.
- **`camel-quarkus-jetty` extension not published in 3.20.** Workaround in `build.gradle.kts` pulls `org.apache.camel:camel-jetty:4.8.0` directly. Documented inline; flagged for reviewers because a future Quarkus bump may publish the extension and silently change classpath shape.
- **rust-camel-cli wrapper SIGKILL limitation.** Wrapper's own docstring notes that the v1/T2 harness M1 pattern (SIGKILL after marker) orphans the camel child because bash traps can't catch SIGKILL. The wrapper script's docstring proposes two fixes for Task 5: (a) SIGTERM-then-grace-then-SIGKILL, or (b) signal the wrapper's process group via `setsid`. Either must land before M1 measurement.
- **Marker count guard.** All 4 Java/Quarkus fixtures use `AtomicBoolean` one-shot guards; the rust-camel probe tasks return after the first emit. The smoke's `marker_count != 1` assertion is satisfied by construction; no re-emit risk under normal supervision restarts.

## 5. Review fixes — BLOCKED on missing listener-bound hook (escalation to e_gpt)

Status: BLOCKED. No production-rust-camel change was made; the analysis
below is the deliverable for the conductor's `@experts/e_gpt` dispatch.

### Root cause (architectural)

The Java fixtures use `EventNotifierSupport` subscribed to Camel's
`RouteStarted` event, which fires AFTER `JettyServer.start()` returns.
`JettyServer.start()` is synchronous w.r.t. listener bind — so the
Java `RouteStarted` event IS a reliable listener-bound signal.

rust-camel's `RuntimeEvent::RouteStarted` looks like the equivalent
but IS NOT, because the consumer start is asynchronous. Trace:

1. `CamelContext::start()` (`crates/camel-core/src/context.rs:656`)
   → `start_context` (`context_lifecycle.rs:43`) iterates route_ids
   and calls `runtime.execute(StartRoute)`.
2. `execute(StartRoute)` → `handle_lifecycle_start`
   (`commands.rs:385`): Phase 1 persist Starting; Phase 2
   `apply_runtime_lifecycle` → `execution.start_route(...)`; Phase 2a
   `aggregate.confirm_start()` publishes `RouteStarted`.
3. `RuntimeExecutionAdapter::start_route` → `controller.start_route`
   → actor → `DefaultRouteController::start_route`
   (`route_controller_trait.rs:30`).
4. `DefaultRouteController::start_route` calls
   `consumer_management::spawn_consumer_task(...)`
   (`route_controller_trait.rs:384`, also aggregate variant at
   `route_controller.rs:901`). `spawn_consumer_task`
   (`consumer_management.rs:41`) wraps the consumer.start() in
   `tokio::spawn` and returns a `JoinHandle` immediately. The
   function then logs `"Route started"` and returns `Ok(())`.
5. So `apply_runtime_lifecycle` returns `Ok` BEFORE
   `consumer.start()` actually runs. `RouteStarted` is published
   immediately afterward. The actual `TcpListener::bind` happens
   later in the spawned task; if it fails, the error is logged as
   `"Consumer error: ..."` (`consumer_management.rs:53`) and a
   `CrashNotification` is sent — but `ctx.start()` has already
   returned `Ok` to `main`.

### Empirical confirmation

This matches the committed smoke log for rust-camel-lib
(`benchmarks/scenarios/http-server/smoke/logs/rust-camel-lib.log`):
the wrapper's TCP probe succeeded (against an orphan), the marker
fired, and the child process later logged `Consumer error: Endpoint
creation failed: Failed to bind 0.0.0.0:8080: Address already in use`.
If `ctx.start()` had waited for `consumer.start()`, the bind error
would have propagated and the marker task (spawned after
`ctx.start().await?`) would never have started.

### What hook I expected

A public, synchronous-after-bind signal analogous to Camel's
`RouteStarted`-after-`JettyServer.start()`. Specifically one of:

(a) `CamelContext::start()` returns `Ok` only after every consumer's
    `start()` has completed (bind included) — this is what the
    existing fixture's docstring in
    `benchmarks/scenarios/http-server/rust-camel-lib/src/main.rs`
    CLAIMS, but the actual code in `consumer_management.rs:41-78`
    does not honour it.
(b) A public event subscription on `CamelContext`, e.g.
    `ctx.subscribe_events() -> Receiver<RuntimeEvent>`, coupled
    with a NEW `RuntimeEvent::ListenerBound { route_id, addr }`
    variant fired from inside `HttpConsumer::start()` after the
    `TcpListener::bind` future resolves.
(c) An HTTP-component-specific ready signal, paralleling
    `camel-kafka/src/consumer.rs:86 pub fn ready_signal(&self) ->
    Arc<Notify>`. HttpConsumer currently exposes nothing similar.

### What's missing

- `CamelContext::start()` is non-blocking w.r.t. consumer bind.
- `RuntimeEvent::RouteStarted` publishes before consumer.start()
  runs to completion (so subscribing to it would not fix the bug).
- `EventPublisherPort` (`runtime_ports.rs`) is internal —
  `CamelContext` exposes no public event stream.
- `HttpConsumer` exposes no ready/listener-bound callback. Only
  `camel-kafka` has `ready_signal()`; `camel-http` does not.
- `ReadinessGate` (`camel-api/src/platform.rs:147`) is a
  Kubernetes-style platform probe, not a per-route listener probe.

### Why TCP probe is structurally broken (and cannot be patched)

A TCP probe from inside the same process cannot distinguish its own
listener from a stale orphan bound to the same port — this is the
exact failure the Task 1 review flagged. Any "improvement" (PID
inspection via `/proc/net/tcp` + `/proc/<pid>/fd`, `ss -tlnp` parse,
etc.) is shell/OS-specific, fragile, and not the framework hook
the spec §4.10 calls for.

### Why YAML DSL fix (option (b) in Step 3) is also blocked

`camel run` + YAML DSL funnels through the same
`CamelContext::start()` → async-consumer-spawn path. There is no
route-start hook in the YAML DSL because there is no synchronous
route-start hook in the runtime. Adding `--emit-ready-marker` to the
CLI would have the same problem: the CLI calls `ctx.start().await`
(`crates/camel-cli/src/commands/run.rs:6` "Start context" block),
which returns Ok before bind.

### Proposed options for e_gpt consultation

Pick ONE; all are production-source changes the brief explicitly
excludes from this task's scope, hence BLOCKED rather than silent
fallback.

1. **Make `spawn_consumer_task` await `consumer.start()` before
   returning** (smallest diff; honours the contract the fixture
   docstring already assumes). Risk: long-blocking starts (e.g.
   Kafka consumer group join) would block `ctx.start()` — may need
   a per-component ready-vs-started split.
2. **Add `RuntimeEvent::RouteConsumerStarted { route_id }` fired
   from inside `spawn_consumer_task` after `consumer.start().await`
   resolves Ok**, plus a public
   `CamelContext::subscribe_runtime_events() -> Receiver<RuntimeEvent>`.
   Fixture would then await a `RouteConsumerStarted` for `bench-http`
   before emitting `BENCH_ROUTE_READY`.
3. **Add a `ready_signal()` to `HttpConsumer`** (paralleling
   `camel-kafka`) and expose it via the consumer trait so the
   fixture can await the per-port Notify. Most surgical for HTTP,
   but requires touching the consumer trait surface.

### State of the worktree

No code changes; the working tree is clean at `b1dd5f78`. The two
rust-camel fixtures retain their existing TCP-probe markers
(known-broken per Concerns §4) pending the e_gpt decision. Once a
hook lands, both fixtures converge on the same fix shape and the
smoke re-run can proceed.

## 6. Review fixes (post-rc-w1u9) — markers decoupled from orphan-port race

Section 5 documented the BLOCKED state: `CamelContext::start()` returned
Ok before the HTTP consumer's `TcpListener::bind` resolved, so no
in-process or wrapper-side signal was 1:1 coupled to the listener-bind
moment. Both rust-camel fixtures carried a TCP probe that could
silently succeed against a stale orphan while the child died with
EADDRINUSE. bd `rc-w1u9` (Option E: `ConsumerStartupMode::Explicit`
handshake) landed in production source on 2026-07-19
(`crates/components/camel-component-api/src/consumer.rs`,
`crates/components/camel-http/src/lib.rs`,
`crates/camel-core/src/lifecycle/adapters/consumer_management.rs`).
This section documents the fixture convergence on the new hook.

### 6.1 Production guarantee (rc-w1u9)

- `HttpConsumer::startup_mode()` returns `Explicit`
  (`crates/components/camel-http/src/lib.rs:1569`).
- `HttpConsumer::start()` performs `TcpListener::bind` + axum spawn +
  route registration, THEN calls `ctx.mark_ready()` (`camel-http/
  src/lib.rs:1289`). A bind failure makes `start()` return `Err`,
  which `spawn_consumer_task` propagates via
  `startup_for_task.mark_failed(e.to_string())`
  (`consumer_management.rs:91-98`).
- `CamelContext::start()` awaits the `StartupReceiver` for every
  Explicit consumer; it returns `Ok` ONLY after `mark_ready()` has
  fired and returns `Err` on `mark_failed()` (bind failure surfaced
  as a proper startup error, not a silent background log).
- Net effect: when `ctx.start()` returns `Ok`, the HTTP listener IS
  bound and accepting connections. When it returns `Err` (e.g.
  EADDRINUSE from a stale orphan), the CLI exits with code 1.

### 6.2 rust-camel-lib marker mechanism (revised)

- **File**: `benchmarks/scenarios/http-server/rust-camel-lib/src/main.rs:146-150`.
- **Hook**: direct `println!("BENCH_ROUTE_READY {unix_ms}")`
  immediately after `ctx.start().await?` returns Ok. No TCP probe, no
  sibling timer, no background task — the line executes only when
  control reaches it, which under rc-w1u9 means the listener is
  genuinely bound. The previous `emit_listener_bound_marker`
  function (which polled 127.0.0.1:8080 in a tokio task) has been
  deleted.
- **Orphan-port behavior**: if a stale orphan holds 8080, the
  child's `ctx.start()` returns `Err` (EADDRINUSE propagated via
  `mark_failed`), the `?` operator propagates it as a process
  exit, and the marker line is never reached. The harness sees no
  marker → hard failure (correct outcome).
- **Smoke log evidence** (`smoke/rust-camel-lib.log`):
  `CamelContext started` at `16:04:43.418242Z`; `BENCH_ROUTE_READY
  1784477083418` immediately after (epoch-ms 1784477083418 =
  16:04:43.418Z — same millisecond); `BENCH_HTTP_REQUEST received`
  + `id=1` on POST. No EADDRINUSE, marker count = 1.

### 6.3 rust-camel-cli wrapper marker mechanism (revised)

- **File**: `benchmarks/scenarios/http-server/rust-camel-cli/
  http-server-cli-wrapper.sh` (full rewrite of the marker loop).
- **Hook**: the wrapper greps the child's stdout for the literal
  `"CamelContext started"` line (emitted by `camel_core::lifecycle
  ::application::context_lifecycle` after `ctx.start()` returns Ok,
  via the tracing subscriber installed by `camel_config::context_ext`
  with `with_writer(std::io::stdout)`). When the wrapper sees that
  line, it emits `BENCH_ROUTE_READY <unix_ms>`.
- **Why this is correct under rc-w1u9**: the child's tracing
  subscriber emits `"CamelContext started"` ONLY after `ctx.start()`
  returns Ok, which under rc-w1u9 means THIS child's listener is
  bound. The line cannot appear if a stale orphan holds 8080 (in
  that case the child exits with `Failed to start CamelContext:
  … Address already in use …` and the wrapper detects the exit
  before the success line appears).
- **Rejected alternatives** (documented in the wrapper's header
  docstring):
  - DSL `log:` step at route start: fires lazily on first request,
    not at bind time → marker would not be listener-bound.
  - TCP probe + child-liveness check: racy — there is a brief
    startup window where the child is alive but its bind has not
    yet failed, during which the probe could hit the orphan.
    Empirically confirmed: first wrapper revision used this
    approach and emitted a spurious marker under orphan-port
    regression test.
- **Orphan-port behavior**: empirically verified — see §6.5.
- **Smoke log evidence** (`smoke/rust-camel-cli.log`):
  `CamelContext started` at `16:04:43.608881Z`; wrapper emits
  `BENCH_ROUTE_READY 1784477083616` immediately after (epoch-ms
  1784477083616 = 16:04:43.616Z — ~7ms later, attributable to the
  wrapper's 10ms poll interval + tail-follow latency);
  `BENCH_HTTP_REQUEST received` on POST. No EADDRINUSE, marker
  count = 1.

### 6.4 Java artifact markers (unchanged)

The two Java Pair A artifacts (`camel-standalone-dsl`,
`camel-quarkus-dsl-native`) were never affected by the orphan-port
race — their markers are emitted from inside the JVM via
`EventNotifierSupport` on the `RouteStarted` event, which fires
after the Jetty listener is bound. rc-w1u9 changed nothing about
their marker mechanism. Smoke re-run confirms both still emit
exactly one marker and respond 200/pong.

### 6.5 Smoke re-run results (all 4 mandated artifacts + 4 extras)

Fresh `bash benchmarks/scenarios/http-server/smoke/run.sh` with
verified-clean port 8080 (`pkill -9 -f 'camel|http-server|jetty|
quarkus'` + `ss -tlnp | grep :8080` returns empty) before launch:

| Artifact | Marker line | POST /bench | EADDRINUSE | Result |
|---|---|---|---|---|
| rust-camel-lib | `BENCH_ROUTE_READY 1784477083418` | 200/pong | none | PASS |
| rust-camel-cli | `BENCH_ROUTE_READY 1784477083616` | 200/pong | none | PASS |
| camel-standalone-dsl | `BENCH_ROUTE_READY 1784477084213` | 200/pong | none | PASS |
| camel-quarkus-dsl-native | `BENCH_ROUTE_READY 1784477087227` | 200/pong | none | PASS |
| camel-standalone-yaml (extra) | `BENCH_ROUTE_READY 1784477084943` | 200/pong | none | PASS |
| camel-quarkus-dsl (extra) | `BENCH_ROUTE_READY 1784477085902` | 200/pong | none | PASS |
| camel-quarkus-yaml (extra) | `BENCH_ROUTE_READY 1784477086907` | 200/pong | none | PASS |
| camel-quarkus-yaml-native (extra) | `BENCH_ROUTE_READY 1784477087412` | 200/pong | none | PASS |

Summary: 8/8 PASS, marker count = 1 each, no EADDRINUSE.

### 6.6 Orphan-port regression test (rust-camel-cli wrapper)

Verified the wrapper refuses to emit a marker when a stale orphan
holds 8080:

1. Launch the rust-camel-lib binary as orphan (binds 8080, waits
   for ctrl_c). Confirm via `ss -tlnp` that the orphan is bound.
2. Run the wrapper against the held port.
3. The wrapper's child (`camel run`) attempts to bind 8080, fails
   with `Address already in use (os error 98)` (propagated via
   rc-w1u9's `mark_failed`), `ctx.start()` returns `Err`, the CLI
   exits with code 1.
4. The wrapper detects the child exit before seeing
   `"CamelContext started"`, prints the failure signature
   (`Failed to start CamelContext: … EADDRINUSE …`), and exits 1
   without emitting any marker.

Result: `wrapper exit code: 1`, **no `BENCH_ROUTE_READY` in
wrapper output**, diagnostic correctly identifies the bind error.
This is the exact failure mode §5's "Why TCP probe is structurally
broken" section warned about, now correctly detected and reported
as a hard failure instead of silently corrupting the measurement.

### 6.7 Quality gates

```
cargo fmt --check --all                                PASS
cargo clippy --workspace --all-features                PASS (0 warnings)
  --exclude camel-cli --exclude camel-component-kafka
  --exclude security-keycloak --exclude security-wasm-policy
  -- -D warnings
cargo xtask lint-unwrap                                PASS (0 violations)
cargo xtask lint-secrets                               PASS (0 violations)
cargo xtask lint-log-levels                            PASS (0 violations)
```

### 6.8 Commits

- `fix(bench/v3): use ctx.start() blocking for rust-camel-lib marker`
  — deletes the TCP-probe task, emits the marker directly after
  `ctx.start().await?`.
- `fix(bench/v3): wait on child 'CamelContext started' for CLI marker`
  — replaces the wrapper's TCP probe with a stdout-line wait that
  is structurally decoupled from any orphan-port race.
- `test(bench/v3): refresh T3 smoke logs after marker fix` —
  captures the post-fix smoke evidence for §6.5.

### 6.9 Comparison table — marker mechanism × 4 mandated artifacts

| Artifact | Mechanism | Source | Listener-bound? | Orphan-safe? |
|---|---|---|---|---|
| camel-standalone-dsl | EventNotifier on RouteStarted (JVM) | `BenchRoute.configure()` | YES (Jetty bind before route start) | YES (JVM in-process) |
| camel-quarkus-dsl-native | EventNotifier on RouteStarted (native) | `BenchRoute.configure()` | YES | YES |
| rust-camel-lib | rc-w1u9 `ctx.start()` blocks on mark_ready; direct `println!` after | `rust-camel-lib/src/main.rs:150` | YES (rc-w1u9 handshake) | YES (bind failure → ctx.start Err → no marker) |
| rust-camel-cli | wrapper waits for child's `"CamelContext started"` stdout line | `http-server-cli-wrapper.sh` (post-rc-w1u9 rewrite) | YES (line emitted only after bind) | YES (child exits before line on bind failure) |

## 7. Final review fixes (Task 3 second-pass review)

This section resolves the 1 Important + 4 Minor reviewer findings filed
against §6's commit set. Smoke re-run is in §7.4 (all 8 artifacts emit
`BENCH_HTTP_REQUEST id=1` except rust-camel-cli, which is BLOCKED on
the YAML DSL — see §7.1.7 and bd `rc-5gcu`).

### 7.1 Important — `BENCH_HTTP_REQUEST id=<n>` emitted on 7/8 artifacts

The Task 3 brief mandates the `id=<n>` per-request signal for harness
cross-check with loadgen records (M2 Protocol A correlation per spec
§4.4-4.5). §6's fixtures emitted either the static literal
`BENCH_HTTP_REQUEST received` (no id) or nothing at all. Only
`rust-camel-lib` was correct.

#### 7.1.1 camel-standalone-dsl — App.java

- File: `benchmarks/scenarios/http-server/camel-standalone/camel-standalone-dsl/src/main/java/com/rustcamel/bench/App.java`.
- **Before**: `from(...).log("BENCH_HTTP_REQUEST received").setBody(constant("pong"))`.
- **After**: added `AtomicLong requestId = new AtomicLong(0)` field
  and inserted `.process(e -> System.out.println("BENCH_HTTP_REQUEST id=" + requestId.incrementAndGet()))`
  between `.log(...)` and `.setBody(...)`. Processor runs BEFORE
  `setBody` so loadgen sees the log during request processing.
- Also fixes M2 (atomic guard) — replaces the raw
  `boolean[] markerEmitted = {false}` with
  `AtomicBoolean markerEmitted` + `compareAndSet(false, true)`,
  matching the Quarkus pattern (BenchRoute.java).

#### 7.1.2 camel-standalone-yaml — AppYaml.java + routes.yaml

- Files: `AppYaml.java`, `src/main/resources/routes.yaml`, `pom.xml`.
- **Before**: route was `log + setBody`. AppYaml.java used the
  non-atomic `boolean[] markerEmitted = {false}` pattern.
- **After**:
  - AppYaml.java: (a) atomic guard replaced (M2 fix); (b) new
    `RequestIdGenerator` static inner class with an `AtomicLong id`
    field and a `next()` method that prints `BENCH_HTTP_REQUEST id=<n>`;
    (c) `context.getRegistry().bind("requestIdGenerator", new RequestIdGenerator())`
    in `initCamelContext()` override so the YAML DSL `bean:` step
    resolves the bean at route-start time.
  - routes.yaml: inserted `- to: { uri: "bean:requestIdGenerator", parameters: { method: "next" } }`
    between the `log:` and `setBody:` steps. Canonical Apache Camel
    `bean:` component URI form (not the YAML DSL `bean:` step's
    nested-name form, which Apache Camel 4.8 rejects with
    `Unsupported field: name`).
  - pom.xml: added `org.apache.camel:camel-bean:4.8.0` dependency
    (the `bean:` component URI requires the camel-bean artifact;
    without it, route start fails with
    `NoSuchEndpointException: No endpoint could be found for: bean://requestIdGenerator`).

#### 7.1.3 camel-quarkus-dsl — BenchRoute.java

- File: `BenchRoute.java` (shared by `camel-quarkus-dsl` and
  `camel-quarkus-dsl-native` via sourceSets).
- **Before**: `from(...).log("BENCH_HTTP_REQUEST received").setBody(constant("pong"))`.
- **After**: added `private static final AtomicLong REQUEST_ID = new AtomicLong(0);`
  and `.process(e -> System.out.println("BENCH_HTTP_REQUEST id=" + REQUEST_ID.incrementAndGet()))`
  between `.log(...)` and `.setBody(...)`. The static field survives
  CDI bean re-injection (defensive; in practice the bean is
  `@ApplicationScoped` so a single instance lives for the process
  lifetime).

#### 7.1.4 camel-quarkus-yaml — RouteStartedMarker.java + routes.yaml

- Files: `RouteStartedMarker.java`, `src/main/resources/camel/routes.yaml`,
  `build.gradle.kts`, `camel-quarkus-yaml-native/build.gradle.kts`.
- **Before**: RouteStartedMarker.java was a marker-only dummy
  (empty `configure()` body, just registered the EventNotifier).
  Route was `log + setBody`.
- **After**:
  - RouteStartedMarker.java: (a) `@Inject RequestIdGenerator` CDI
    field; (b) new `@ApplicationScoped @RegisterForReflection
    static class RequestIdGenerator` with `AtomicLong` + `next()`;
    (c) `bindBean()` helper called from both `@PostConstruct` and
    `configure()` (one-shot guard via `BEAN_BOUND` AtomicBoolean)
    that binds the CDI bean into the CamelContext's registry under
    name `requestIdGenerator`. `@RegisterForReflection` is required
    for Substrate VM — without it, native-image's closed-world
    analysis drops the `next` method as unreachable and the bean
    invocation fails with `MethodNotFoundException` at runtime
    (only the native variant was affected; JVM sibling worked
    without it because HotSpot resolves the method reflectively).
  - routes.yaml: inserted `- to: { uri: "bean:requestIdGenerator", parameters: { method: "next" } }`.
  - build.gradle.kts (JVM + native): added
    `implementation("org.apache.camel.quarkus:camel-quarkus-bean")`.

#### 7.1.5 camel-quarkus-dsl-native

No source change — `camel-quarkus-dsl-native` shares Java source
with `camel-quarkus-dsl` via `sourceSets.java.srcDir`. Native
binary rebuilt with the same NixOS workaround documented in
`benchmarks/spike-results.md` Spike 1B
(`-Dquarkus.native.additional-build-args=-H:NativeLinkerOption=-L/nix/store/dbz6pb9g67kpgpl95k8d85kzpxm1c32p-zlib-1.3.2/lib`).
~84s native-image compile.

#### 7.1.6 camel-quarkus-yaml-native

No direct source change — shares Java source + camel/routes.yaml
with `camel-quarkus-yaml`. Native binary rebuilt; required the
`@RegisterForReflection` annotation on `RequestIdGenerator` (see
§7.1.4). ~82s native-image compile.

#### 7.1.7 rust-camel-cli (BLOCKED) — bd `rc-5gcu`

- Files: `rust-camel-cli/routes/http-server.yaml`,
  `rust-camel-cli/Camel.toml` (M1 comment fix only — no id emission
  change).
- **Status**: **BLOCKED on YAML DSL capability gap**. The rust-camel
  YAML DSL cannot easily emit a per-request incrementing id:
  - No `process:` step (only Apache Camel Java DSL has closure-based
    process steps; rust-camel's RouteBuilder API has `.process(...)` but
    YAML DSL doesn't expose it as a step kind).
  - The `bean:` step exists in the YAML grammar
    (`crates/camel-dsl/src/yaml.rs:74` DeclarativeStepKind::Bean) but
    the CLI's bean registry only loads WASM beans from
    `camel_config.beans` (Camel.toml `[beans]` section pointing to
    `.wasm` files in `plugins_dir`) — there's no built-in/native bean
    registration path from TOML config.
  - No Simple-language counter variable available (camel-simple has
    no `CamelCounter` / `exchangeProperty.requestId`).
  - The `script:` step exists but supports language/source
    evaluation with no monotonic counter state.
- **Per reviewer instruction**: "If YAML DSL cannot easily emit
  incrementing id, escalate via BLOCKED — do NOT silently omit."
  Filed bd `rc-5gcu` (priority 2, discovered-from `rc-2vxg`) with
  three unblocking options: (1) WASM counter bean, (2) built-in CLI
  counter bean (production change), (3) wrapper-side counter via
  stdout rewrite.
- The static `log: "BENCH_HTTP_REQUEST received"` line is retained
  for spec §4.10 "per-request log emission" observability. The
  load-bearing `id=<n>` signal is missing from this artifact only.

### 7.2 Minor M1 — stale "liveness-gated TCP probe" comment

- Files: `rust-camel-cli/routes/http-server.yaml:7-13`,
  `rust-camel-cli/Camel.toml:11-16`.
- **Before**: comments said the wrapper emits the marker via a
  "liveness-gated TCP probe" (a pre-rc-w1u9 description that was
  never updated when the wrapper mechanism changed).
- **After**: replaced with the post-rc-w1u9 description — the
  wrapper detects the child's `CamelContext started` stdout line
  (the deterministic rc-w1u9 post-bind signal).

### 7.3 Minor M2 — non-atomic marker guard

- Files: `App.java:62,72,82`, `AppYaml.java:39,50,53`.
- **Before**: `final boolean[] markerEmitted = {false}` (non-atomic;
  visibility not guaranteed across threads if the EventNotifier
  fires from a non-main thread).
- **After**: `final AtomicBoolean markerEmitted = new AtomicBoolean(false)`
  + `markerEmitted.compareAndSet(false, true)` on emit. Matches the
  pattern already used by the Quarkus fixtures (`BenchRoute.java`,
  `RouteStartedMarker.java`).

### 7.4 Minor M3 — route shape asymmetry (POST-only vs any-method)

- **Status**: documented (no code change). The Java fixtures use
  `jetty:http://0.0.0.0:8080/bench?httpMethodRestrict=POST` (POST-
  only); rust-camel's HTTP consumer doesn't expose method restriction
  in the YAML DSL — the URI is bare `http://0.0.0.0:8080/bench` and
  accepts any method. Adding restriction would require a production
  change (out of T3 scope).
- **Behavioral impact**: zero. The harness (Task 5) only POSTs to
  `/bench`, so the asymmetry is invisible to the measurement. The
  M2 Protocol B load generator is also POST-only.
- This note satisfies the brief's documentation requirement.

### 7.5 Minor M5 — `nc` preflight check in smoke runner

- File: `benchmarks/scenarios/http-server/smoke/run.sh:13-19`.
- **Before**: `printf 'POST ...' | nc 127.0.0.1 8080` at line 67
  with no check that `nc` exists. NixOS hosts don't always ship
  netcat by default — silent failure with confusing error.
- **After**: added `command -v nc >/dev/null 2>&1 || { echo "FAIL: nc (netcat) required..."; exit 1; }`
  at the top of the script (after `set -uo pipefail`).

### 7.6 Smoke runner — `verify_request_id` assertion

Extended `smoke/run.sh` with a new `verify_request_id()` function
that asserts each artifact's smoke log contains `BENCH_HTTP_REQUEST id=1`
(smoke sends exactly 1 request per artifact). rust-camel-cli is
excluded (BLOCKED per §7.1.7) — the check verifies the static
`received` line is present instead and emits a `PASS (partial)`
line referencing bd `rc-5gcu`.

### 7.7 Smoke re-run results (all 8 artifacts)

Smoke run after the fixes:

| Artifact | Marker | POST /bench | id=1 | Status |
|---|---|---|---|---|
| rust-camel-lib | 1 | 200/pong | YES | PASS |
| rust-camel-cli | 1 | 200/pong | NO (static `received` only — BLOCKED bd rc-5gcu) | PASS (partial) |
| camel-standalone-dsl | 1 | 200/pong | YES | PASS |
| camel-standalone-yaml | 1 | 200/pong | YES | PASS |
| camel-quarkus-dsl | 1 | 200/pong | YES | PASS |
| camel-quarkus-yaml | 1 | 200/pong | YES | PASS |
| camel-quarkus-dsl-native | 1 | 200/pong | YES | PASS |
| camel-quarkus-yaml-native | 1 | 200/pong | YES | PASS |

Summary: 8 pass, 0 fail. The `rust-camel-cli` row counts as PASS
(partial) — the marker + POST contract is met; the per-request id
emission is BLOCKED on the YAML DSL capability gap (bd `rc-5gcu`).

### 7.8 Quality gates (post-fixes)

```
cargo fmt --check --all                                  PASS
cargo clippy --workspace --all-features                  PASS (Finished in 1m 51s)
  --exclude camel-cli --exclude camel-component-kafka
  --exclude security-keycloak --exclude security-wasm-policy
  -- -D warnings
cargo xtask lint-unwrap                                  PASS (0 violations)
cargo xtask lint-secrets                                 PASS (0 violations)
cargo xtask lint-log-levels                              PASS (0 violations)
```

### 7.9 Related bd issues

- `rc-5gcu` (NEW, priority 2, bug) — T3 rust-camel-cli YAML cannot
  emit `BENCH_HTTP_REQUEST id=<n>` counter. Filed as
  discovered-from `rc-2vxg`. Three unblocking options documented
  (WASM bean / built-in counter bean / wrapper-side counter).
- `rc-8ysn` (existing, priority 2, bug) — T3 wrapper SIGKILL
  orphans camel child (M4 SIGKILL orphans, filed by conductor).
  Out of T3 scope — Task 5 entrance criterion.

### 7.10 Commits

- `fix(bench/v3): emit BENCH_HTTP_REQUEST id=<n> on T3 artifacts`
  — Java + YAML + build-deps + smoke verify + nc preflight + M1
  comment + M3 doc note.
- `docs(bench/v3): T3 final review fixes report` — this section.
